#[path = "../tests/support/performance.rs"]
mod performance;

use std::{future::Future, path::Path, pin::Pin, process::Command, sync::Arc};

use hivegui::datasource::workflow_store::{NodeType, WorkflowGraph, WorkflowNode};
use hivegui::datasource::{
    store::{Store, StoreOpenOptions},
    tool_store::{ToolInput, ToolKind as PersistedToolKind, ToolSource, ToolStore},
};
use hivegui::runtime::tool_adapter::{
    PersistedToolExecutor, ToolExecutionContext, ToolTargetFuture, ToolTargetRunner,
};
use hivegui::runtime::{CancelHandle, WorkflowExecutor, WorkflowNodeExecutor};
use serde_json::Value;

use performance::{
    BenchmarkReport, BenchmarkRunError, ComparisonOutcome, EnvironmentFingerprint,
    ReleaseMatrixReport, TOOL_DISPATCH_ID, TOOL_DISPATCH_OPERATIONS_PER_SAMPLE, TargetSpec,
    WORKFLOW_100_NODE_ID, baseline_path, evaluate_benchmark_gate, measure_local_async,
    measure_local_async_checked_batched, regression_exception_path, run_release_matrix_with,
    source_revision, target_specs,
};

fn main() {
    let mut args = std::env::args().skip(1).peekable();

    if let Some(arg) = args.peek() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                return;
            }
            "--targets" => {
                print_targets();
                return;
            }
            "--target" => {
                let _ = args.next();
                if let Some(target_id) = args.next() {
                    print_target(target_id.as_str());
                    return;
                }
                eprintln!("--target requires an argument: target id");
                print_usage();
                std::process::exit(2);
            }
            "--baseline-path" => {
                let _ = args.next();
                if let Some(target_id) = args.next() {
                    print_baseline_path(target_id.as_str());
                    return;
                }
                eprintln!("--baseline-path requires an argument: target id");
                print_usage();
                std::process::exit(2);
            }
            "--exception-path" => {
                let _ = args.next();
                if let Some(target_id) = args.next() {
                    print_exception_path(target_id.as_str());
                    return;
                }
                eprintln!("--exception-path requires an argument: target id");
                print_usage();
                std::process::exit(2);
            }
            "--source-revision" => {
                print_source_revision();
                return;
            }
            "--manifest" => {
                print_manifest();
                return;
            }
            "--run-matrix" => {
                run_release_matrix();
                return;
            }
            "--run" => {
                let _ = args.next();
                let target_id = args.next();
                let mut expected_source = None;
                while let Some(option) = args.next() {
                    match option.as_str() {
                        "--expected-source" => {
                            let Some(source) = args.next() else {
                                eprintln!(
                                    "--expected-source requires an argument: source revision"
                                );
                                print_usage();
                                std::process::exit(2);
                            };
                            expected_source = Some(source);
                        }
                        _ => {
                            eprintln!("Unknown --run argument: {option}");
                            print_usage();
                            std::process::exit(2);
                        }
                    }
                }
                run_benchmarks(target_id.as_deref(), expected_source.as_deref());
                return;
            }
            _ => {
                eprintln!("Unknown argument: {}", arg);
                print_usage();
                std::process::exit(2);
            }
        }
    }

    print_manifest();
}

fn print_usage() {
    let message = concat!(
        "Usage: cargo bench -p hivegui --bench local_runtime [--help|-h] [--manifest] [--targets] [--target <id>] [--baseline-path <id>] [--exception-path <id>] [--source-revision] [--run-matrix] [--run <id> [--expected-source <revision>]]\n",
        "\n",
        "  --help|-h         Show this help text\n",
        "  --manifest        Print the shared benchmark manifest (default)\n",
        "  --targets         Print supported benchmark target identifiers\n",
        "  --target <id>     Print target metadata by id\n",
        "  --baseline-path <id>\n",
        "                   Print expected baseline path for a given target\n",
        "  --exception-path <id>\n",
        "                   Print the canonical regression-exception sidecar path\n",
        "  --source-revision\n",
        "                   Print the deterministic dirty-worktree source revision\n",
        "  --run-matrix     Execute the source-owned 13-target release matrix\n",
        "  --run <id>       Execute one real target, print its JSON report, and compare its baseline\n"
    );
    println!("{}", message);
}

fn run_release_matrix() {
    let report = match source_revision(repository_root()) {
        Ok(expected_source) => match std::env::current_exe() {
            Ok(executable) => run_release_matrix_with(
                &executable,
                &expected_source,
                || match source_revision(repository_root()) {
                    Ok(source) => source,
                    Err(error) => {
                        eprintln!("cannot probe release matrix source revision: {error}");
                        format!("source revision unavailable: {error}")
                    }
                },
                |child_executable, args| {
                    Command::new(child_executable)
                        .args(args)
                        .status()
                        .map_err(|error| error.to_string())?
                        .code()
                        .ok_or_else(|| "child terminated without an exit code".to_owned())
                },
            ),
            Err(error) => ReleaseMatrixReport::failed_setup(
                &expected_source,
                format!("cannot resolve current benchmark executable: {error}"),
            ),
        },
        Err(error) => ReleaseMatrixReport::failed_setup(
            "unavailable",
            format!("cannot capture release matrix source revision: {error}"),
        ),
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("serialize release matrix report")
    );
    if report.status == "failed" {
        std::process::exit(1);
    }
}

fn print_manifest() {
    let environment = EnvironmentFingerprint::capture();
    let targets = target_specs();
    let manifest = serde_json::json!({
        "schema_version": performance::BASELINE_SCHEMA_VERSION,
        "fixture_version": performance::FIXTURE_VERSION,
        "environment": environment,
        "status": "pending_first_approved_green",
        "targets": targets.iter().map(|target| serde_json::json!({
            "target": target,
            "baseline_path": baseline_path(target, &environment),
            "exception_path": regression_exception_path(target, &environment),
        })).collect::<Vec<_>>(),
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&manifest).expect("serialize benchmark manifest")
    );
    eprintln!(
        "No placeholder timing was recorded: each target's owning task supplies its first approved Green run."
    );
}

fn print_targets() {
    for target in target_specs() {
        println!("{}", target.id);
    }
}

fn print_target(target_id: &str) {
    if let Some(target) = target_by_id(target_id) {
        println!(
            "{}",
            serde_json::to_string_pretty(&target).expect("serialize target")
        );
        return;
    }

    eprintln!("target not found: {}", target_id);
    print_targets();
    std::process::exit(2);
}

fn print_baseline_path(target_id: &str) {
    if let Some(target) = target_by_id(target_id) {
        let environment = EnvironmentFingerprint::capture();
        println!("{}", baseline_path(&target, &environment).display());
        return;
    }

    eprintln!("target not found: {}", target_id);
    print_targets();
    std::process::exit(2);
}

fn print_exception_path(target_id: &str) {
    if let Some(target) = target_by_id(target_id) {
        let environment = EnvironmentFingerprint::capture();
        println!(
            "{}",
            regression_exception_path(&target, &environment).display()
        );
        return;
    }

    eprintln!("target not found: {}", target_id);
    print_targets();
    std::process::exit(2);
}

fn print_source_revision() {
    match source_revision(repository_root()) {
        Ok(revision) => println!("{revision}"),
        Err(error) => {
            eprintln!("cannot capture source revision: {error}");
            std::process::exit(1);
        }
    }
}

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("hivegui manifest directory must be nested under the repository root")
}

fn target_by_id(id: &str) -> Option<TargetSpec> {
    target_specs()
        .iter()
        .find(|target| target.id == id)
        .cloned()
}

/// Execute one or more benchmark targets and print each report. The
/// process exits non-zero if a target fails its absolute p95 budget, an
/// existing baseline cannot be loaded, or comparison blocks. Baseline
/// approval and file creation remain explicit review actions.
fn run_benchmarks(target_id: Option<&str>, expected_source: Option<&str>) {
    let revision = match source_revision(repository_root()) {
        Ok(revision) => revision,
        Err(error) => {
            eprintln!("cannot capture source revision: {error}");
            std::process::exit(1);
        }
    };
    if let Some(expected_source) = expected_source
        && revision != expected_source
    {
        eprintln!(
            "expected source revision mismatch: expected {expected_source}, actual {revision}"
        );
        std::process::exit(1);
    }
    let as_of = chrono::Utc::now().date_naive();
    eprintln!("source revision: {revision}");
    eprintln!("performance gate date (UTC): {as_of}");
    let environment = EnvironmentFingerprint::capture();
    let targets: Vec<TargetSpec> = target_specs()
        .into_iter()
        .filter(|target| target_id.is_none_or(|id| target.id == id))
        .collect();

    if targets.is_empty() {
        eprintln!("no benchmark target matched: {target_id:?}");
        print_targets();
        std::process::exit(2);
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build benchmark tokio runtime");

    let mut failed = false;
    for target in targets {
        match runtime.block_on(run_target(&target, &environment)) {
            Ok(Some(report)) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("serialize benchmark report")
                );
                eprintln!(
                    "REPORT {}: p50={}ns p95={}ns p99={}ns (absolute budget p95<={}ns)",
                    target.id,
                    report.percentiles_ns.p50,
                    report.percentiles_ns.p95,
                    report.percentiles_ns.p99,
                    target.p95_budget_ns
                );

                // Versioned baseline comparison (T005). A brand-new
                // entry reports PendingBaseline until a reviewer
                // approves its first Green run; thereafter any tracked
                // percentile regression >10% blocks.
                let baseline_path = baseline_path(&target, &environment);
                let exception_path = regression_exception_path(&target, &environment);
                eprintln!("  baseline path: {}", baseline_path.display());
                eprintln!("  exception path: {}", exception_path.display());
                let comparison = evaluate_benchmark_gate(
                    &report,
                    &revision,
                    &baseline_path,
                    &exception_path,
                    as_of,
                );
                match comparison {
                    Ok(result) => {
                        for regression in &result.regressions {
                            eprintln!(
                                "  regression {:?}: baseline={}ns current={}ns",
                                regression.percentile,
                                regression.baseline_ns,
                                regression.current_ns
                            );
                        }
                        if let Some(rejection) = &result.exception_rejection {
                            eprintln!("  exception: rejected ({rejection})");
                        }
                        match result.outcome {
                            ComparisonOutcome::Passed => {
                                eprintln!("  performance gate: passed");
                            }
                            ComparisonOutcome::PendingBaseline => {
                                eprintln!(
                                    "  performance gate: pending (first approved Green establishes its baseline)"
                                );
                            }
                            ComparisonOutcome::Blocked => {
                                eprintln!("  performance gate: BLOCKED");
                                failed = true;
                            }
                            ComparisonOutcome::ApprovedException => {
                                eprintln!("  performance gate: approved exception");
                            }
                        }
                    }
                    Err(error) => {
                        eprintln!("  performance gate: invalid ({error})");
                        failed = true;
                    }
                }
            }
            Ok(None) => {
                eprintln!("SKIP {}: operation not implemented yet", target.id);
            }
            Err(error) => {
                eprintln!("ERROR {}: {error}", target.id);
                failed = true;
            }
        }
    }

    if failed {
        std::process::exit(1);
    }
}

/// Run a single target and return its report (or `None` when the
/// target's operation is not implemented yet).
async fn run_target(
    target: &TargetSpec,
    environment: &EnvironmentFingerprint,
) -> Result<Option<BenchmarkReport>, Box<dyn std::error::Error>> {
    if let Some(report) = performance::run_target(target, environment).await? {
        return Ok(Some(report));
    }

    match target.id.as_str() {
        TOOL_DISPATCH_ID => {
            let temporary_root = tempfile::tempdir()?;
            let store = Store::open_local(StoreOpenOptions::new(
                temporary_root.path().join("hivegui.db"),
                temporary_root.path().join("plugins"),
            ))
            .await?;
            let schema = r#"{"type":"object"}"#;
            let now = chrono::Utc::now().to_rfc3339();
            let function_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO functions (identifier, name, description, kind, input_schema, \
                 output_schema, plugin_id, plugin_export, category_id, required_capabilities, \
                 created_at, updated_at) VALUES ('tool_dispatch_benchmark_function', \
                 'Tool dispatch benchmark Function', '', 'builtin', ?, ?, NULL, NULL, NULL, \
                 NULL, ?, ?) RETURNING id",
            )
            .bind(schema)
            .bind(schema)
            .bind(&now)
            .bind(&now)
            .fetch_one(store.pool())
            .await?;
            let tools = ToolStore::new(store.pool().clone())?;
            let tool = tools
                .create(ToolInput::for_write(
                    "tool_dispatch_benchmark".to_string(),
                    "Tool dispatch benchmark".to_string(),
                    "Persisted production Tool dispatch fixture".to_string(),
                    PersistedToolKind::FunctionWrap,
                    ToolSource::Workspace,
                    false,
                    Some(function_id),
                    None,
                    schema.to_string(),
                    schema.to_string(),
                    None,
                    Some(r#"["log.emit"]"#.to_string()),
                )?)
                .await?;
            let executor =
                PersistedToolExecutor::new(store.pool().clone(), Arc::new(NoopPersistedToolRunner));
            let tool_id = tool.id();
            let _fixture_lifetime = (store, temporary_root);
            measure_local_async_checked_batched(
                target.clone(),
                environment.clone(),
                TOOL_DISPATCH_OPERATIONS_PER_SAMPLE,
                move || {
                    let executor = executor.clone();
                    async move {
                        executor
                            .execute(
                                tool_id,
                                serde_json::json!({}),
                                ToolExecutionContext::new(vec!["log.emit".to_string()]),
                            )
                            .await
                            .map_err(|error| BenchmarkRunError::Operation(error.to_string()))?;
                        Ok(())
                    }
                },
            )
            .await
            .map(Some)
            .map_err(Into::into)
        }
        WORKFLOW_100_NODE_ID => {
            let graph = build_100_node_graph();
            let executor = Arc::new(WorkflowExecutor::new(NoopNodeExecutor));
            measure_local_async(target.clone(), environment.clone(), move || {
                let graph = graph.clone();
                let executor = Arc::clone(&executor);
                async move {
                    // Validation + topological scheduling of a 100-node
                    // no-op DAG; node execution is a no-op and excluded.
                    let _ = executor
                        .execute(&graph, Value::Null, CancelHandle::new())
                        .await;
                }
            })
            .await
            .map(Some)
            .map_err(Into::into)
        }
        _ => Ok(None),
    }
}

struct NoopPersistedToolRunner;

impl ToolTargetRunner for NoopPersistedToolRunner {
    fn execute_function(
        &self,
        _function_id: i64,
        input: Value,
        _granted_capabilities: Vec<String>,
    ) -> ToolTargetFuture {
        Box::pin(async move { Ok(input) })
    }

    fn execute_workflow(
        &self,
        _workflow_id: i64,
        input: Value,
        _granted_capabilities: Vec<String>,
    ) -> ToolTargetFuture {
        Box::pin(async move { Ok(input) })
    }
}

/// Build the fixed 100-node linear no-op DAG: one `start_node`, 98
/// `function_node` instances, and one `end_node`.
fn build_100_node_graph() -> WorkflowGraph {
    let mut builder = WorkflowGraph::builder().name("bench-100-node");
    builder = builder.node(WorkflowNode::new("start", NodeType::Start));
    for index in 0..98 {
        builder = builder.node(WorkflowNode::new(format!("n{index}"), NodeType::Function));
    }
    builder = builder.node(WorkflowNode::new("end", NodeType::End));
    builder = builder.edge("start", "n0");
    for index in 0..97 {
        builder = builder.edge(format!("n{index}"), format!("n{}", index + 1));
    }
    builder = builder.edge("n97", "end");
    builder.build()
}

/// No-op workflow node executor used by the benchmark. Every node
/// resolves to a null value immediately, so the measured time reflects
/// validation and scheduling, not user Function/Plugin execution.
struct NoopNodeExecutor;

impl WorkflowNodeExecutor for NoopNodeExecutor {
    fn execute(
        &self,
        _node: WorkflowNode,
        _input: Value,
        _cancel: CancelHandle,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send>> {
        Box::pin(async { Ok(Value::Null) })
    }
}
