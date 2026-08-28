mod support;

use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use chrono::NaiveDate;
use support::{
    CapturedHttpServer, FIXTURE_DEVICE_KEY, FaultInjector, MockHttpResponse, TestWorkspace,
    performance::{
        AGENT_ACTION_DISPATCH_ID, AGENT_CRUD_ID, AGENT_SEARCH_OPERATIONS_PER_SAMPLE,
        AGENT_SEARCH_PAGE_ID, ApprovedRegression, BASELINE_SCHEMA_VERSION, BaselineApproval,
        BenchmarkBaseline, BenchmarkReport, ComparisonOutcome, ComparisonResult,
        CompatibilityError, EnvironmentFingerprint, FIXTURE_VERSION, Percentile, PercentilesNs,
        REGRESSION_EXCEPTION_SCHEMA_VERSION, RELEASE_MATRIX_ID, RegressionException,
        TOOL_DISPATCH_ID, TOOL_DISPATCH_OPERATIONS_PER_SAMPLE, WORKFLOW_100_NODE_ID, baseline_path,
        compare_to_baseline, evaluate_benchmark_gate, load_baseline, load_regression_exception,
        measure_local_async_batched, measure_local_async_checked_batched,
        regression_exception_path, release_matrix_invocations, release_matrix_target_ids,
        run_release_matrix_with, run_target, source_revision, target_specs,
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

    assert_eq!(targets.len(), 13);
    assert_eq!(targets[0].id, AGENT_ACTION_DISPATCH_ID);
    assert_eq!(targets[0].owner_task, "T122");
    assert_eq!(targets[0].p95_budget_ns, 200_000_000);
    assert_eq!(targets[1].id, AGENT_CRUD_ID);
    assert_eq!(targets[1].owner_task, "T115");
    assert_eq!(targets[1].p95_budget_ns, 1_000_000_000);
    assert_eq!(targets[2].id, AGENT_SEARCH_PAGE_ID);
    assert_eq!(targets[2].owner_task, "T115");
    assert_eq!(targets[2].p95_budget_ns, 500_000_000);
    assert_eq!(targets[3].id, TOOL_DISPATCH_ID);
    assert_eq!(targets[3].owner_task, "T103");
    assert_eq!(targets[3].p95_budget_ns, 50_000_000);
    assert_eq!(targets[4].id, WORKFLOW_100_NODE_ID);
    assert_eq!(targets[4].owner_task, "T093");
    assert_eq!(targets[4].p95_budget_ns, 100_000_000);
    assert_eq!(targets[4].workflow_node_count, Some(100));
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
fn release_matrix_target_ids_follow_the_reviewed_interference_minimizing_order() {
    // This reviewed order reduces preceding heavy-load interference. The new
    // matrix remains falsifiable and does not claim that host affinity is controlled.
    let expected = [
        "agent_action_dispatch",
        "tool_dispatch_batched_v2",
        "conversation_list_recent_batched_v2",
        "workflow_100_node_noop",
        "conversation_session_bundle_batched_v2",
        "agent_search_page",
        "conversation_running_recovery",
        "conversation_retention_cleanup",
        "function_crud",
        "tool_crud",
        "agent_crud",
        "function_search_page_pair_v2",
        "tool_search_page_pair_v2",
    ];
    let actual: [&str; 13] = release_matrix_target_ids();
    assert_eq!(
        actual, expected,
        "reviewed interference-minimizing matrix order drifted"
    );

    let unique = actual
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        unique.len(),
        actual.len(),
        "matrix target IDs must be unique"
    );
    let matrix_set = unique
        .into_iter()
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    let manifest_set = target_specs()
        .into_iter()
        .map(|target| target.id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        matrix_set, manifest_set,
        "matrix and benchmark manifest must own exactly the same target set"
    );
}

#[test]
fn release_matrix_invocations_bind_exact_executable_source_and_single_target() {
    assert_eq!(RELEASE_MATRIX_ID, "hivegui_local_runtime_release_matrix_v1");
    let executable = PathBuf::from("/tmp/exact-local-runtime-benchmark");
    let expected_source = "git:1111111111111111111111111111111111111111+hivegui-source-v1:2222222222222222222222222222222222222222222222222222222222222222";
    let invocations = release_matrix_invocations(&executable, expected_source);
    let expected = release_matrix_target_ids()
        .into_iter()
        .map(|target_id| {
            serde_json::json!({
                "executable": executable.as_path(),
                "args": ["--run", target_id, "--expected-source", expected_source],
            })
        })
        .collect::<Vec<_>>();

    assert_eq!(
        serde_json::to_value(invocations).expect("serialize matrix invocations"),
        serde_json::Value::Array(expected)
    );
}

#[test]
fn release_matrix_collects_nonzero_and_spawn_failures_before_failing_aggregate() {
    let executable = PathBuf::from("/tmp/exact-local-runtime-benchmark");
    let expected_source = "git:1111111111111111111111111111111111111111+hivegui-source-v1:2222222222222222222222222222222222222222222222222222222222222222";
    let source_probes = std::cell::Cell::new(0);
    let mut invocations = Vec::<serde_json::Value>::new();
    let report = run_release_matrix_with(
        &executable,
        expected_source,
        || {
            source_probes.set(source_probes.get() + 1);
            expected_source.to_owned()
        },
        |child_executable: &Path, args: &[String]| {
            invocations.push(serde_json::json!({
                "executable": child_executable,
                "args": args,
            }));
            match invocations.len() {
                3 => Ok(3),
                7 => Err("spawn sentinel".to_owned()),
                _ => Ok(0),
            }
        },
    );

    assert_eq!(
        source_probes.get(),
        2,
        "source must be probed at start and end"
    );
    assert_eq!(
        invocations.len(),
        13,
        "failures must not truncate the matrix"
    );
    let expected_invocations = release_matrix_invocations(&executable, expected_source);
    assert_eq!(
        serde_json::Value::Array(invocations),
        serde_json::to_value(expected_invocations).expect("serialize expected matrix invocations")
    );

    let expected_targets = release_matrix_target_ids()
        .into_iter()
        .enumerate()
        .map(|(index, target_id)| {
            let ordinal = index + 1;
            match ordinal {
                3 => serde_json::json!({
                    "ordinal": ordinal,
                    "target_id": target_id,
                    "status": "failed",
                    "exit_code": 3,
                    "error": null,
                }),
                7 => serde_json::json!({
                    "ordinal": ordinal,
                    "target_id": target_id,
                    "status": "failed",
                    "exit_code": null,
                    "error": "spawn sentinel",
                }),
                _ => serde_json::json!({
                    "ordinal": ordinal,
                    "target_id": target_id,
                    "status": "passed",
                    "exit_code": 0,
                    "error": null,
                }),
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        serde_json::to_value(report).expect("serialize matrix report"),
        serde_json::json!({
            "schema_version": 1,
            "id": RELEASE_MATRIX_ID,
            "source_revision": expected_source,
            "affinity": "inherited/uncontrolled",
            "source_stable": true,
            "status": "failed",
            "targets": expected_targets,
        })
    );
}

#[test]
fn release_matrix_passes_only_when_all_children_pass_and_source_is_stable() {
    let executable = PathBuf::from("/tmp/exact-local-runtime-benchmark");
    let expected_source = "git:1111111111111111111111111111111111111111+hivegui-source-v1:2222222222222222222222222222222222222222222222222222222222222222";
    let source_probes = std::cell::Cell::new(0);
    let launches = std::cell::Cell::new(0);
    let report = run_release_matrix_with(
        &executable,
        expected_source,
        || {
            source_probes.set(source_probes.get() + 1);
            expected_source.to_owned()
        },
        |_child_executable: &Path, _args: &[String]| {
            launches.set(launches.get() + 1);
            Ok(0)
        },
    );

    assert_eq!(source_probes.get(), 2);
    assert_eq!(launches.get(), 13);
    let expected_targets = release_matrix_target_ids()
        .into_iter()
        .enumerate()
        .map(|(index, target_id)| {
            serde_json::json!({
                "ordinal": index + 1,
                "target_id": target_id,
                "status": "passed",
                "exit_code": 0,
                "error": null,
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        serde_json::to_value(report).expect("serialize passing matrix report"),
        serde_json::json!({
            "schema_version": 1,
            "id": RELEASE_MATRIX_ID,
            "source_revision": expected_source,
            "affinity": "inherited/uncontrolled",
            "source_stable": true,
            "status": "passed",
            "targets": expected_targets,
        })
    );
}

#[test]
fn release_matrix_fails_when_source_changes_between_start_and_end() {
    let executable = PathBuf::from("/tmp/exact-local-runtime-benchmark");
    let expected_source = "git:1111111111111111111111111111111111111111+hivegui-source-v1:2222222222222222222222222222222222222222222222222222222222222222";
    let changed_source = "git:1111111111111111111111111111111111111111+hivegui-source-v1:3333333333333333333333333333333333333333333333333333333333333333";
    let mut source_probes = [expected_source, changed_source].into_iter();
    let report = run_release_matrix_with(
        &executable,
        expected_source,
        || {
            source_probes
                .next()
                .expect("matrix must probe source exactly twice")
                .to_owned()
        },
        |_child_executable: &Path, _args: &[String]| Ok(0),
    );
    assert!(
        source_probes.next().is_none(),
        "matrix must probe source at both envelope boundaries"
    );
    let encoded = serde_json::to_value(report).expect("serialize source-drift matrix report");
    assert_eq!(encoded["schema_version"], 1);
    assert_eq!(encoded["id"], RELEASE_MATRIX_ID);
    assert_eq!(encoded["source_revision"], expected_source);
    assert_eq!(encoded["affinity"], "inherited/uncontrolled");
    assert_eq!(encoded["source_stable"], false);
    assert_eq!(encoded["status"], "failed");
    assert_eq!(
        encoded["targets"]
            .as_array()
            .expect("machine-readable matrix target results")
            .len(),
        13
    );
}

#[test]
fn canonical_release_baselines_match_all_current_targets() {
    let environment = EnvironmentFingerprint {
        os: "linux".into(),
        architecture: "x86_64".into(),
        rustc: "rustc 1.97.1 (8bab26f4f 2026-07-14)".into(),
        build_profile: "release".into(),
        cpu_model: "12th Gen Intel(R) Core(TM) i9-12900K".into(),
        logical_cpus: 20,
    };
    let targets = target_specs();
    assert_eq!(targets.len(), 13);

    let mut baseline_count = 0;
    for target in targets {
        let path = baseline_path(&target, &environment);
        assert!(
            path.is_file(),
            "missing canonical baseline: {}",
            path.display()
        );
        let baseline = load_baseline(&path)
            .unwrap_or_else(|error| {
                panic!("invalid canonical baseline {}: {error}", path.display())
            })
            .unwrap_or_else(|| panic!("missing canonical baseline: {}", path.display()));
        baseline_count += 1;
        assert_eq!(
            baseline.report.target,
            target,
            "canonical baseline target drifted at {}",
            path.display()
        );
        assert_eq!(baseline.report.schema_version, BASELINE_SCHEMA_VERSION);
        assert_eq!(baseline.report.fixture_version, FIXTURE_VERSION);
        assert_eq!(baseline.report.environment, environment);
        assert_eq!(baseline.report.sample_count, target.measured_samples);
    }
    assert_eq!(baseline_count, 13);
}

#[test]
fn release_matrix_cli_owns_machine_readable_fresh_child_execution() {
    let runtime = include_str!("../benches/local_runtime.rs");
    assert!(
        runtime.contains("\"--run-matrix\""),
        "the canonical final matrix requires one source-owned --run-matrix entrypoint"
    );
    let start = runtime
        .find("fn run_release_matrix(")
        .expect("source-owned final release matrix driver");
    let remaining = &runtime[start..];
    let end = remaining[1..]
        .find("\nfn ")
        .map(|offset| offset + 1)
        .unwrap_or(remaining.len());
    let branch = &remaining[..end];
    assert!(
        branch.contains("std::env::current_exe()")
            && branch.contains("run_release_matrix_with(")
            && branch.contains("Command::new(child_executable)")
            && branch.contains(".args(args)")
            && branch.contains(".status()")
            && branch.contains("serde_json::to_string_pretty(&report)"),
        "CLI matrix mode must print its envelope and launch fresh exact-executable children"
    );
    let failure_gate = branch
        .find("if report.status")
        .expect("matrix report failure condition");
    let gate_open = branch[failure_gate..]
        .find('{')
        .map(|offset| offset + failure_gate)
        .expect("matrix report failure block");
    let gate_close = branch[gate_open..]
        .find("\n    }")
        .map(|offset| offset + gate_open)
        .expect("matrix report failure block end");
    let failure_exit = branch
        .find("std::process::exit(1);")
        .expect("matrix aggregate failure exit");
    assert_eq!(
        branch.matches("std::process::exit(1);").count(),
        1,
        "matrix CLI must have exactly one aggregate failure exit"
    );
    assert!(
        branch[failure_gate..gate_open]
            .to_ascii_lowercase()
            .contains("failed")
            && gate_open < failure_exit
            && failure_exit < gate_close,
        "matrix CLI must exit non-zero only inside the failed-report condition"
    );

    assert!(
        runtime.contains("\"--expected-source\"")
            && runtime.contains("run_benchmarks(target_id.as_deref(), expected_source.as_deref())"),
        "matrix children must forward their expected source into single-target execution"
    );
    let benchmark_start = runtime
        .find("fn run_benchmarks(")
        .expect("single-target benchmark runner");
    let benchmark_branch = &runtime[benchmark_start..];
    let mismatch = benchmark_branch
        .find("expected source revision mismatch")
        .expect("fail-closed expected-source diagnostic");
    let exit = benchmark_branch[mismatch..]
        .find("std::process::exit(1);")
        .map(|offset| offset + mismatch)
        .expect("fail-closed expected-source exit");
    let target_execution = benchmark_branch
        .find("run_target(&target")
        .expect("production target execution");
    assert!(
        mismatch < exit && exit < target_execution,
        "a child must reject source mismatch before timing or evaluation"
    );

    let quickstart = include_str!("../../../specs/011-hivegui-standalone-mode/quickstart.md");
    assert!(
        quickstart.contains(
            "cargo +1.97.1 bench --locked -p hivegui --bench local_runtime -- --run-matrix"
        ),
        "quickstart must invoke the source-owned final matrix driver"
    );
    assert!(
        !quickstart.contains("for target in \\") && !quickstart.contains("--run \"$target\""),
        "quickstart must not duplicate or drift the source-owned matrix order"
    );
}

#[tokio::test]
async fn agent_action_dispatch_target_executes_the_production_boundary() {
    let target = target_specs()
        .into_iter()
        .find(|target| target.id == AGENT_ACTION_DISPATCH_ID)
        .expect("agent action dispatch target");

    let report = run_target(&target, &fixture_environment())
        .await
        .expect("agent action dispatch benchmark must execute")
        .expect("agent action dispatch benchmark must not be skipped");

    assert_eq!(report.target.id, AGENT_ACTION_DISPATCH_ID);
    assert_eq!(report.sample_count, 100);
    assert!(
        report.percentiles_ns.p95 <= target.p95_budget_ns,
        "Agent action dispatch p95={}ns exceeds {}ns",
        report.percentiles_ns.p95,
        target.p95_budget_ns
    );
}

#[test]
fn tool_dispatch_benchmark_uses_the_persisted_production_boundary() {
    let targets = target_specs();
    assert_eq!(targets.len(), 13);
    let target = targets
        .iter()
        .find(|target| target.id == TOOL_DISPATCH_ID)
        .expect("canonical Tool dispatch target");
    assert_eq!(TOOL_DISPATCH_ID, "tool_dispatch_batched_v2");
    assert_eq!(
        target.timing_boundary,
        "one per-dispatch normalized sample from a fixed batch of 1024 identical persisted Function-wrap Tool requests, each measured from persisted Tool execution acceptance through Tool/Function target lookup, input-schema and Capability validation, local no-op target return, output-schema validation, and validated result materialization"
    );
    let environment = fixture_environment();
    let v2_baseline = baseline_path(target, &environment);
    assert_eq!(
        v2_baseline
            .parent()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str()),
        Some("tool_dispatch_batched_v2")
    );
    assert_ne!(
        v2_baseline
            .parent()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str()),
        Some("tool_dispatch")
    );

    let source = include_str!("../benches/local_runtime.rs");
    let start = source
        .find("TOOL_DISPATCH_ID =>")
        .expect("tool_dispatch benchmark branch");
    let end = source[start..]
        .find("WORKFLOW_100_NODE_ID =>")
        .map(|offset| start + offset)
        .expect("next benchmark branch");
    let branch = &source[start..end];

    assert!(
        branch.contains("PersistedToolExecutor"),
        "T103 must measure the persisted production Tool boundary before the local target starts"
    );
    assert!(
        !branch.contains("LocalToolAdapter::default_in_memory"),
        "T103 must not substitute an already-classified in-memory adapter for persisted Tool validation"
    );
    assert!(
        branch.contains("measure_local_async_checked_batched")
            && branch.contains("TOOL_DISPATCH_OPERATIONS_PER_SAMPLE")
            && !branch.contains("measure_local_async_batched("),
        "sub-millisecond Tool dispatch samples must use the reviewed checked fixed batch"
    );
    assert!(
        branch.contains("ToolExecutionContext::new(vec![")
            && !branch.contains("ToolExecutionContext::new(Vec::new())")
            && !branch.contains("let _ = executor"),
        "the dispatch runner must exercise a granted Capability and propagate every execution error"
    );
}

#[tokio::test]
async fn batched_async_measurement_normalizes_each_sample_and_executes_every_operation() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let target = target_specs()
        .into_iter()
        .find(|target| target.id == TOOL_DISPATCH_ID)
        .expect("tool dispatch target");
    let expected_operations = (target.warmup_iterations + target.measured_samples) * 32;
    let calls = Arc::new(AtomicUsize::new(0));
    let measured_calls = Arc::clone(&calls);

    let report = measure_local_async_batched(target, fixture_environment(), 32, move || {
        let measured_calls = Arc::clone(&measured_calls);
        async move {
            measured_calls.fetch_add(1, Ordering::Relaxed);
        }
    })
    .await
    .expect("batched measurement");

    assert_eq!(report.sample_count, 100);
    assert_eq!(calls.load(Ordering::Relaxed), expected_operations);
}

#[tokio::test]
async fn checked_tool_dispatch_batch_propagates_the_first_operation_error() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    assert_eq!(TOOL_DISPATCH_OPERATIONS_PER_SAMPLE, 1024);
    let target = target_specs()
        .into_iter()
        .find(|target| target.id == TOOL_DISPATCH_ID)
        .expect("Tool dispatch v2 target");
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);
    let error = measure_local_async_checked_batched(
        target,
        fixture_environment(),
        TOOL_DISPATCH_OPERATIONS_PER_SAMPLE,
        move || {
            let observed_calls = Arc::clone(&observed_calls);
            async move {
                observed_calls.fetch_add(1, Ordering::Relaxed);
                Err::<(), _>(support::performance::BenchmarkRunError::Operation(
                    "checked-dispatch-sentinel".into(),
                ))
            }
        },
    )
    .await
    .expect_err("the checked batch must propagate the first dispatch error");

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        error.to_string(),
        "benchmark operation failed: checked-dispatch-sentinel"
    );
}

#[test]
fn agent_search_benchmark_repeats_each_scheduled_step_in_a_normalized_batch() {
    assert_eq!(AGENT_SEARCH_OPERATIONS_PER_SAMPLE, 16);
    let source = include_str!("support/performance.rs");
    let start = source
        .find("async fn run_agent_search_page(")
        .expect("Agent search benchmark");
    let end = source[start..]
        .find("struct AgentManagementBenchmarkFixture")
        .map(|offset| start + offset)
        .expect("Agent benchmark fixture boundary");
    let branch = &source[start..end];
    assert!(
        branch.contains("measure_local_async_checked_batched")
            && branch.contains("AGENT_SEARCH_OPERATIONS_PER_SAMPLE")
            && branch.contains("/ AGENT_SEARCH_OPERATIONS_PER_SAMPLE"),
        "each deterministic Agent search step must be repeated in one fixed normalized batch"
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
fn regression_exception_schema_round_trips_bound_sidecar() {
    let baseline = fixture_baseline(PercentilesNs {
        p50: 100,
        p95: 100,
        p99: 100,
    });
    let encoded = serde_json::json!({
        "schema_version": 1,
        "target": baseline.report.target,
        "environment": baseline.report.environment,
        "current_source_revision": concat!(
            "git:89abcdef0123456789abcdef0123456789abcdef",
            "+hivegui-source-v1:",
            "89abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567"
        ),
        "baseline": {
            "approval": baseline.approval,
            "percentiles_ns": baseline.report.percentiles_ns,
        },
        "approved_regressions": [
            {
                "percentile": "p95",
                "baseline_ns": 100,
                "observed_current_ns": 111,
                "approved_max_current_ns": 125,
            }
        ],
        "approval": {
            "signer": "performance-reviewer",
            "approved_at": "2026-08-21",
            "reason": "documented temporary variance",
            "impact_scope": "function_crud on the exact release environment",
            "review_due": "2026-09-21",
        },
    });

    let exception: RegressionException = serde_json::from_value(encoded.clone())
        .expect("decode the versioned, context-bound regression exception sidecar");
    assert_eq!(
        serde_json::to_value(exception).expect("encode regression exception sidecar"),
        encoded
    );
}

#[test]
fn approved_regression_schema_records_observation_and_upper_bound() {
    assert_eq!(REGRESSION_EXCEPTION_SCHEMA_VERSION, 1);
    let encoded = serde_json::json!({
        "percentile": "p95",
        "baseline_ns": 100,
        "observed_current_ns": 111,
        "approved_max_current_ns": 125,
    });
    let approved: ApprovedRegression =
        serde_json::from_value(encoded.clone()).expect("decode one approved regression");

    assert_eq!(
        serde_json::to_value(approved).expect("encode one approved regression"),
        encoded
    );
}

#[test]
fn regression_exception_path_is_manifest_anchored_and_next_to_baseline() {
    let target = target_specs()[1].clone();
    let environment = fixture_environment();
    let baseline = baseline_path(&target, &environment);
    let exception = regression_exception_path(&target, &environment);

    assert!(
        exception.is_absolute(),
        "exception path must not depend on cwd"
    );
    assert_eq!(exception, baseline.with_extension("exception.json"));
}

#[test]
fn regression_exception_loader_only_treats_not_found_as_absent() {
    let directory = tempfile::tempdir().expect("create exception fixture directory");
    let missing = directory.path().join("missing.exception.json");
    assert_eq!(
        load_regression_exception(&missing).expect("missing exception means no approval"),
        None
    );

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
    let encoded = regression_exception_json(
        &current,
        &baseline,
        vec![approved_regression_json(Percentile::P95, 100, 111, 125)],
    );
    let path = directory.path().join("valid.exception.json");
    fs::write(
        &path,
        serde_json::to_vec(&encoded).expect("encode exception fixture"),
    )
    .expect("write valid exception fixture");
    let loaded = load_regression_exception(&path)
        .expect("load valid exception")
        .expect("valid exception exists");

    assert_eq!(
        serde_json::to_value(loaded).expect("encode loaded exception"),
        encoded
    );
}

#[test]
fn regression_exception_loader_fails_closed_for_invalid_existing_files() {
    let directory = tempfile::tempdir().expect("create exception fixture directory");

    let malformed = directory.path().join("malformed.exception.json");
    fs::write(&malformed, b"{not-json").expect("write malformed exception");
    assert!(
        load_regression_exception(&malformed).is_err(),
        "an existing malformed exception must fail closed"
    );

    let unreadable_as_file = directory.path().join("exception-directory.json");
    fs::create_dir(&unreadable_as_file).expect("create non-file exception path");
    assert!(
        load_regression_exception(&unreadable_as_file).is_err(),
        "an existing non-file exception path must fail closed"
    );

    #[cfg(unix)]
    {
        let dangling = directory.path().join("dangling.exception.json");
        std::os::unix::fs::symlink("missing-target.json", &dangling)
            .expect("create dangling exception symlink");
        assert!(
            load_regression_exception(&dangling).is_err(),
            "an existing dangling exception symlink must fail closed"
        );
    }

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
    let mut unknown_field = regression_exception_json(
        &current,
        &baseline,
        vec![approved_regression_json(Percentile::P95, 100, 111, 125)],
    );
    unknown_field
        .as_object_mut()
        .expect("exception JSON object")
        .insert("unreviewed_field".into(), serde_json::json!(true));
    let unknown_path = directory.path().join("unknown-field.exception.json");
    fs::write(
        &unknown_path,
        serde_json::to_vec(&unknown_field).expect("encode unknown-field fixture"),
    )
    .expect("write unknown-field exception");
    assert!(
        load_regression_exception(&unknown_path).is_err(),
        "unknown exception fields must fail closed"
    );
}

#[test]
fn benchmark_gate_blocks_regression_when_exception_sidecar_is_missing() {
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
    let result = evaluate_fixture_gate(
        &current,
        &baseline,
        None,
        NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
    );

    assert_eq!(result.outcome, ComparisonOutcome::Blocked);
    assert_eq!(result.regressions.len(), 1);
    assert_eq!(result.regressions[0].percentile, Percentile::P95);
}

#[test]
fn benchmark_gate_accepts_an_exact_context_with_a_subset_of_unique_approvals() {
    let baseline = fixture_baseline(PercentilesNs {
        p50: 100,
        p95: 100,
        p99: 100,
    });
    let current = fixture_report(PercentilesNs {
        p50: 100,
        p95: 120,
        p99: 100,
    });
    let exception = regression_exception_json(
        &current,
        &baseline,
        vec![
            approved_regression_json(Percentile::P95, 100, 111, 125),
            approved_regression_json(Percentile::P99, 100, 115, 130),
        ],
    );
    let result = evaluate_fixture_gate(
        &current,
        &baseline,
        Some(&exception),
        NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
    );

    assert_eq!(result.outcome, ComparisonOutcome::ApprovedException);
    assert_eq!(result.regressions.len(), 1);
    assert_eq!(result.regressions[0].percentile, Percentile::P95);
    assert_eq!(result.regressions[0].baseline_ns, 100);
    assert_eq!(result.regressions[0].current_ns, 120);
    assert_eq!(result.exception_rejection, None);
}

#[test]
fn benchmark_gate_rejects_mismatched_context_or_regression_binding() {
    let baseline = fixture_baseline(PercentilesNs {
        p50: 100,
        p95: 100,
        p99: 100,
    });
    let current = fixture_report(PercentilesNs {
        p50: 100,
        p95: 120,
        p99: 100,
    });
    let valid = regression_exception_json(
        &current,
        &baseline,
        vec![approved_regression_json(Percentile::P95, 100, 111, 125)],
    );
    let mut cases = Vec::new();

    let mut wrong_target = valid.clone();
    wrong_target["target"]["id"] = serde_json::json!("different_target");
    cases.push(("target", current.clone(), wrong_target));

    let mut wrong_environment = valid.clone();
    wrong_environment["environment"]["cpu_model"] = serde_json::json!("different CPU");
    cases.push(("environment", current.clone(), wrong_environment));

    let mut wrong_source = valid.clone();
    wrong_source["current_source_revision"] = serde_json::json!(concat!(
        "git:fedcba9876543210fedcba9876543210fedcba98",
        "+hivegui-source-v1:",
        "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
    ));
    cases.push(("source revision", current.clone(), wrong_source));

    let mut wrong_baseline_approval = valid.clone();
    wrong_baseline_approval["baseline"]["approval"]["reviewer"] =
        serde_json::json!("different-reviewer");
    cases.push((
        "baseline approval",
        current.clone(),
        wrong_baseline_approval,
    ));

    let mut wrong_baseline_source = valid.clone();
    wrong_baseline_source["baseline"]["approval"]["source_revision"] = serde_json::json!(concat!(
        "git:fedcba9876543210fedcba9876543210fedcba98",
        "+hivegui-source-v1:",
        "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
    ));
    cases.push((
        "baseline approval source",
        current.clone(),
        wrong_baseline_source,
    ));

    let mut wrong_baseline_percentiles = valid.clone();
    wrong_baseline_percentiles["baseline"]["percentiles_ns"]["p95"] = serde_json::json!(99);
    cases.push((
        "baseline percentiles",
        current.clone(),
        wrong_baseline_percentiles,
    ));

    let mut wrong_regression_baseline = valid.clone();
    wrong_regression_baseline["approved_regressions"][0]["baseline_ns"] = serde_json::json!(99);
    cases.push((
        "approved regression baseline",
        current.clone(),
        wrong_regression_baseline,
    ));

    let mut duplicate = valid.clone();
    duplicate["approved_regressions"]
        .as_array_mut()
        .expect("approved regressions array")
        .push(approved_regression_json(Percentile::P95, 100, 112, 130));
    cases.push(("duplicate percentile", current.clone(), duplicate));

    let mut wrong_schema = valid.clone();
    wrong_schema["schema_version"] = serde_json::json!(2);
    cases.push(("schema version", current.clone(), wrong_schema));

    let mut new_percentile = current.clone();
    new_percentile.percentiles_ns.p50 = 111;
    cases.push(("unapproved percentile", new_percentile, valid.clone()));

    let mut above_cap = current.clone();
    above_cap.percentiles_ns.p95 = 126;
    cases.push(("current value above cap", above_cap, valid));

    for (label, current, exception) in cases {
        let result = evaluate_fixture_gate(
            &current,
            &baseline,
            Some(&exception),
            NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
        );
        assert_eq!(
            result.outcome,
            ComparisonOutcome::Blocked,
            "{label} must fail closed: {result:?}"
        );
        assert!(
            result.exception_rejection.is_some(),
            "{label} must report why its exception was rejected: {result:?}"
        );
    }
}

#[test]
fn benchmark_gate_rejects_invalid_approval_metadata_and_date_order() {
    let baseline = fixture_baseline(PercentilesNs {
        p50: 100,
        p95: 100,
        p99: 100,
    });
    let current = fixture_report(PercentilesNs {
        p50: 100,
        p95: 120,
        p99: 100,
    });
    let valid = regression_exception_json(
        &current,
        &baseline,
        vec![approved_regression_json(Percentile::P95, 100, 111, 125)],
    );
    let mut cases = Vec::new();

    for field in [
        "signer",
        "approved_at",
        "reason",
        "impact_scope",
        "review_due",
    ] {
        let mut blank = valid.clone();
        blank["approval"][field] = serde_json::json!("   ");
        cases.push((format!("blank {field}"), blank, None));
    }

    let mut malformed_approved_at = valid.clone();
    malformed_approved_at["approval"]["approved_at"] = serde_json::json!("21-08-2026");
    cases.push(("malformed approved_at".into(), malformed_approved_at, None));

    let mut malformed_review_due = valid.clone();
    malformed_review_due["approval"]["review_due"] = serde_json::json!("21-09-2026");
    cases.push(("malformed review_due".into(), malformed_review_due, None));

    let mut future = valid.clone();
    future["approval"]["approved_at"] = serde_json::json!("2026-08-22");
    future["approval"]["review_due"] = serde_json::json!("2026-09-22");
    cases.push(("future approved_at".into(), future, None));

    let mut expired = valid.clone();
    expired["approval"]["review_due"] = serde_json::json!("2026-08-20");
    cases.push(("expired review_due".into(), expired, None));

    let mut reversed_dates = valid.clone();
    reversed_dates["approval"]["approved_at"] = serde_json::json!("2026-08-21");
    reversed_dates["approval"]["review_due"] = serde_json::json!("2026-08-20");
    cases.push((
        "review_due before approved_at".into(),
        reversed_dates,
        Some("precedes approved_at"),
    ));

    let mut invalid_source = valid.clone();
    invalid_source["current_source_revision"] = serde_json::json!("not-a-source-revision");
    cases.push(("malformed source revision".into(), invalid_source, None));

    let mut non_regression_observation = valid.clone();
    non_regression_observation["approved_regressions"][0]["observed_current_ns"] =
        serde_json::json!(110);
    cases.push((
        "observation at the ten-percent limit".into(),
        non_regression_observation,
        None,
    ));

    let mut cap_below_observation = valid;
    cap_below_observation["approved_regressions"][0]["approved_max_current_ns"] =
        serde_json::json!(110);
    cases.push((
        "cap below observed regression".into(),
        cap_below_observation,
        None,
    ));

    for (label, exception, expected_rejection_fragment) in cases {
        let result = evaluate_fixture_gate(
            &current,
            &baseline,
            Some(&exception),
            NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
        );
        assert_eq!(
            result.outcome,
            ComparisonOutcome::Blocked,
            "{label} must fail closed: {result:?}"
        );
        let rejection = result
            .exception_rejection
            .as_deref()
            .unwrap_or_else(|| panic!("{label} must provide an exception rejection reason"));
        if let Some(fragment) = expected_rejection_fragment {
            assert!(
                rejection.contains(fragment),
                "{label} rejection must contain {fragment:?}; got {rejection:?}"
            );
        }
    }
}

#[test]
fn benchmark_gate_never_waives_the_absolute_p95_budget() {
    let mut baseline = fixture_baseline(PercentilesNs {
        p50: 100,
        p95: 100,
        p99: 100,
    });
    baseline.report.target.p95_budget_ns = 110;
    let mut current = fixture_report(PercentilesNs {
        p50: 100,
        p95: 120,
        p99: 100,
    });
    current.target = baseline.report.target.clone();
    let exception = regression_exception_json(
        &current,
        &baseline,
        vec![approved_regression_json(Percentile::P95, 100, 120, 125)],
    );
    let result = evaluate_fixture_gate(
        &current,
        &baseline,
        Some(&exception),
        NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
    );

    assert_eq!(result.outcome, ComparisonOutcome::Blocked);
    assert!(
        result
            .exception_rejection
            .as_deref()
            .is_some_and(|reason| reason.contains("absolute p95 budget")),
        "a signed relative-regression exception must never waive the absolute budget: {result:?}"
    );
}

#[test]
fn missing_baseline_never_waives_the_absolute_p95_budget() {
    let mut current = fixture_report(PercentilesNs {
        p50: 100,
        p95: 111,
        p99: 112,
    });
    current.target.p95_budget_ns = 110;
    let directory = tempfile::tempdir().expect("create missing-baseline fixture directory");
    let result = evaluate_benchmark_gate(
        &current,
        FIXTURE_CURRENT_SOURCE_REVISION,
        &directory.path().join("missing.json"),
        &directory.path().join("missing.exception.json"),
        NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
    )
    .expect("evaluate a first report above its absolute budget");

    assert_eq!(result.outcome, ComparisonOutcome::Blocked);
    assert!(
        result
            .exception_rejection
            .as_deref()
            .is_some_and(|reason| reason.contains("absolute p95 budget")),
        "a missing baseline must not bypass the absolute budget: {result:?}"
    );
}

#[test]
fn benchmark_gate_fails_closed_when_exception_loading_fails() {
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
    let (directory, baseline_path, exception_path) = write_gate_artifacts(&baseline, None);
    fs::write(&exception_path, b"{not-json").expect("write malformed exception sidecar");

    let result = evaluate_benchmark_gate(
        &current,
        FIXTURE_CURRENT_SOURCE_REVISION,
        &baseline_path,
        &exception_path,
        NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
    );
    assert!(
        result.is_err(),
        "a malformed existing exception must fail the gate"
    );
    drop(directory);
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

#[test]
fn baseline_loader_only_treats_not_found_as_pending() {
    let directory = tempfile::tempdir().expect("create baseline fixture directory");
    let missing = directory.path().join("missing.json");
    assert_eq!(
        load_baseline(&missing).expect("missing is a new target"),
        None
    );

    let malformed = directory.path().join("malformed.json");
    fs::write(&malformed, b"{not-json").expect("write malformed baseline");
    assert!(
        load_baseline(&malformed).is_err(),
        "an existing malformed baseline must fail closed"
    );

    let unreadable_as_file = directory.path().join("baseline-directory.json");
    fs::create_dir(&unreadable_as_file).expect("create non-file baseline path");
    assert!(
        load_baseline(&unreadable_as_file).is_err(),
        "an existing unreadable baseline path must fail closed"
    );

    #[cfg(unix)]
    {
        let dangling = directory.path().join("dangling.json");
        std::os::unix::fs::symlink("missing-target.json", &dangling)
            .expect("create dangling baseline symlink");
        assert!(
            load_baseline(&dangling).is_err(),
            "an existing dangling baseline symlink must fail closed"
        );
    }

    let valid = directory.path().join("valid.json");
    let baseline = fixture_baseline(PercentilesNs {
        p50: 10,
        p95: 20,
        p99: 30,
    });
    fs::write(
        &valid,
        serde_json::to_vec(&baseline).expect("encode valid baseline"),
    )
    .expect("write valid baseline");
    assert_eq!(
        load_baseline(&valid).expect("load valid baseline"),
        Some(baseline)
    );
}

#[test]
fn baseline_path_is_manifest_anchored_and_independent_of_process_cwd() {
    let target = target_specs()[0].clone();
    let environment = fixture_environment();

    let path = baseline_path(&target, &environment);
    let expected = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("benches/baselines/v1")
        .join(&target.id)
        .join(format!("{}.json", environment.baseline_key()));

    assert!(path.is_absolute(), "baseline path must not depend on cwd");
    assert_eq!(path, expected);
}

#[test]
fn source_revision_is_deterministic_well_formed_and_read_only() {
    let repository = source_revision_fixture();
    let status_before = git_output(repository.path(), &["status", "--porcelain=v1", "-z"]);

    let first = source_revision(repository.path()).expect("capture source revision");
    let second = source_revision(repository.path()).expect("capture source revision again");

    assert_eq!(first, second);
    let (head, digest) = first
        .strip_prefix("git:")
        .and_then(|value| value.split_once("+hivegui-source-v1:"))
        .expect("versioned source revision format");
    assert_eq!(head.len(), 40);
    assert_eq!(digest.len(), 64);
    assert!(head.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(
        git_output(repository.path(), &["status", "--porcelain=v1", "-z"]),
        status_before,
        "capturing a source revision must not mutate the index or worktree"
    );
}

#[test]
fn source_revision_tracks_dirty_benchmark_sources_and_entry_state() {
    let repository = source_revision_fixture();
    let root = repository.path();
    let initial = source_revision(root).expect("capture initial source revision");

    fs::write(
        root.join("crates/demo/src/lib.rs"),
        b"pub fn value() -> u8 { 2 }\n",
    )
    .expect("change tracked bytes");
    let changed_bytes = source_revision(root).expect("capture changed bytes");
    assert_ne!(changed_bytes, initial);

    fs::write(
        root.join("crates/demo/src/lib.rs"),
        b"pub fn value() -> u8 { 1 }\n",
    )
    .expect("restore tracked bytes");
    fs::write(
        root.join("crates/demo/src/untracked.rs"),
        b"pub const LOCAL: u8 = 1;\n",
    )
    .expect("create untracked source");
    let untracked = source_revision(root).expect("capture untracked source");
    assert_ne!(untracked, initial);
    fs::remove_file(root.join("crates/demo/src/untracked.rs")).expect("remove untracked source");

    fs::remove_file(root.join("third_party/vendor.txt")).expect("delete tracked source");
    let deleted = source_revision(root).expect("capture tracked deletion");
    assert_ne!(deleted, initial);
    fs::write(root.join("third_party/vendor.txt"), b"vendored\n").expect("restore tracked source");

    #[cfg(unix)]
    {
        let executable = root.join("crates/demo/src/lib.rs");
        let mut permissions = fs::metadata(&executable)
            .expect("source metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).expect("set executable bit");
        let executable_revision = source_revision(root).expect("capture executable bit");
        assert_ne!(executable_revision, initial);

        let mut permissions = fs::metadata(&executable)
            .expect("source metadata")
            .permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&executable, permissions).expect("restore executable bit");

        let link = root.join("crates/demo/src/linked.rs");
        fs::remove_file(&link).expect("remove tracked symlink");
        std::os::unix::fs::symlink("../../Cargo.toml", &link).expect("change symlink target");
        let changed_link = source_revision(root).expect("capture symlink target");
        assert_ne!(changed_link, initial);
    }
}

#[test]
fn source_revision_excludes_baselines_specs_docs_and_ignored_files() {
    let repository = source_revision_fixture();
    let root = repository.path();
    let initial = source_revision(root).expect("capture initial source revision");

    for (relative, contents) in [
        (
            "crates/hivegui/benches/baselines/v1/function_crud/local.json",
            b"changed baseline".as_slice(),
        ),
        (
            "crates/hivegui/benches/baselines/v1/function_crud/local.exception.json",
            b"changed regression exception".as_slice(),
        ),
        ("specs/feature/spec.md", b"changed spec".as_slice()),
        ("docs/guide.md", b"changed docs".as_slice()),
        (
            "crates/demo/generated.ignore",
            b"ignored generated source".as_slice(),
        ),
    ] {
        fs::write(root.join(relative), contents).expect("change excluded fixture");
    }

    assert_eq!(
        source_revision(root).expect("capture after excluded changes"),
        initial
    );
}

fn source_revision_fixture() -> tempfile::TempDir {
    let repository = tempfile::tempdir().expect("create source revision repository");
    let root = repository.path();
    for directory in [
        ".cargo",
        "crates/demo/src",
        "crates/hivegui/benches/baselines/v1/function_crud",
        "third_party",
        "specs/feature",
        "docs",
    ] {
        fs::create_dir_all(root.join(directory)).expect("create source revision fixture directory");
    }
    for (relative, contents) in [
        ("Cargo.toml", b"[workspace]\nmembers = []\n".as_slice()),
        ("Cargo.lock", b"version = 4\n".as_slice()),
        (
            "rust-toolchain.toml",
            b"[toolchain]\nchannel = \"1.97.1\"\n".as_slice(),
        ),
        (
            ".cargo/config.toml",
            b"[build]\nincremental = false\n".as_slice(),
        ),
        (
            "crates/demo/src/lib.rs",
            b"pub fn value() -> u8 { 1 }\n".as_slice(),
        ),
        ("third_party/vendor.txt", b"vendored\n".as_slice()),
        (
            "crates/hivegui/benches/baselines/v1/function_crud/local.json",
            b"approved baseline\n".as_slice(),
        ),
        ("specs/feature/spec.md", b"spec\n".as_slice()),
        ("docs/guide.md", b"docs\n".as_slice()),
        (".gitignore", b"*.ignore\n".as_slice()),
    ] {
        fs::write(root.join(relative), contents).expect("write source revision fixture");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("../lib.rs", root.join("crates/demo/src/linked.rs"))
        .expect("create source revision symlink");

    git_ok(root, &["init", "--quiet"]);
    git_ok(root, &["config", "user.name", "HiveGUI test"]);
    git_ok(
        root,
        &["config", "user.email", "hivegui-test@example.invalid"],
    );
    git_ok(root, &["add", "--all"]);
    git_ok(root, &["commit", "--quiet", "-m", "fixture"]);
    repository
}

fn git_ok(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .expect("run fixture git command");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(repository: &Path, arguments: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .expect("run fixture git command");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
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

#[test]
fn comparator_rejects_malformed_baseline_approval_metadata() {
    let report = fixture_report(PercentilesNs {
        p50: 10,
        p95: 20,
        p99: 30,
    });
    let mut baseline = fixture_baseline(report.percentiles_ns);
    baseline.approval.approved_at = "not-a-date".into();
    assert_eq!(
        compare_to_baseline(
            &report,
            Some(&baseline),
            None,
            NaiveDate::from_ymd_opt(2026, 8, 20).unwrap(),
        )
        .unwrap_err(),
        CompatibilityError::BaselineApproval("approved_at")
    );

    baseline.approval.approved_at = "2026-08-20".into();
    baseline.approval.source_revision = "bare-or-arbitrary-revision".into();
    assert_eq!(
        compare_to_baseline(
            &report,
            Some(&baseline),
            None,
            NaiveDate::from_ymd_opt(2026, 8, 20).unwrap(),
        )
        .unwrap_err(),
        CompatibilityError::BaselineApproval("source_revision")
    );
}

const FIXTURE_CURRENT_SOURCE_REVISION: &str = concat!(
    "git:89abcdef0123456789abcdef0123456789abcdef",
    "+hivegui-source-v1:",
    "89abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567"
);

fn approved_regression_json(
    percentile: Percentile,
    baseline_ns: u64,
    observed_current_ns: u64,
    approved_max_current_ns: u64,
) -> serde_json::Value {
    serde_json::json!({
        "percentile": percentile,
        "baseline_ns": baseline_ns,
        "observed_current_ns": observed_current_ns,
        "approved_max_current_ns": approved_max_current_ns,
    })
}

fn regression_exception_json(
    current: &BenchmarkReport,
    baseline: &BenchmarkBaseline,
    approved_regressions: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": REGRESSION_EXCEPTION_SCHEMA_VERSION,
        "target": current.target,
        "environment": current.environment,
        "current_source_revision": FIXTURE_CURRENT_SOURCE_REVISION,
        "baseline": {
            "approval": baseline.approval,
            "percentiles_ns": baseline.report.percentiles_ns,
        },
        "approved_regressions": approved_regressions,
        "approval": {
            "signer": "performance-reviewer",
            "approved_at": "2026-08-21",
            "reason": "documented temporary variance",
            "impact_scope": "the exact benchmark target and release environment",
            "review_due": "2026-09-21",
        },
    })
}

fn write_gate_artifacts(
    baseline: &BenchmarkBaseline,
    exception: Option<&serde_json::Value>,
) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let directory = tempfile::tempdir().expect("create performance gate fixture directory");
    let baseline_path = directory.path().join("fixture.json");
    let exception_path = directory.path().join("fixture.exception.json");
    fs::write(
        &baseline_path,
        serde_json::to_vec(baseline).expect("encode fixture baseline"),
    )
    .expect("write fixture baseline");
    if let Some(exception) = exception {
        fs::write(
            &exception_path,
            serde_json::to_vec(exception).expect("encode fixture exception"),
        )
        .expect("write fixture exception");
    }
    (directory, baseline_path, exception_path)
}

fn evaluate_fixture_gate(
    current: &BenchmarkReport,
    baseline: &BenchmarkBaseline,
    exception: Option<&serde_json::Value>,
    as_of: NaiveDate,
) -> ComparisonResult {
    let (directory, baseline_path, exception_path) = write_gate_artifacts(baseline, exception);
    let result = evaluate_benchmark_gate(
        current,
        FIXTURE_CURRENT_SOURCE_REVISION,
        &baseline_path,
        &exception_path,
        as_of,
    )
    .expect("evaluate fixture performance gate");
    drop(directory);
    result
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
            source_revision: concat!(
                "git:0123456789abcdef0123456789abcdef01234567",
                "+hivegui-source-v1:",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            )
            .into(),
        },
    }
}
