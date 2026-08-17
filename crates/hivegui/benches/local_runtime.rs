#[path = "../tests/support/performance.rs"]
mod performance;

use std::{future::Future, pin::Pin, sync::Arc};

use hivegui::datasource::workflow_store::{NodeType, WorkflowGraph, WorkflowNode};
use hivegui::runtime::tool_adapter::{LocalToolAdapter, ToolCallRequest, ToolKind};
use hivegui::runtime::{CancelHandle, WorkflowExecutor, WorkflowNodeExecutor};
use serde_json::Value;

use performance::{
    BenchmarkBaseline, BenchmarkReport, ComparisonOutcome, EnvironmentFingerprint,
    MeasurementError, TOOL_DISPATCH_ID, TargetSpec, WORKFLOW_100_NODE_ID, baseline_path,
    compare_to_baseline, measure_local_async, target_specs,
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
            "--manifest" => {
                print_manifest();
                return;
            }
            "--run" => {
                let _ = args.next();
                let target_id = args.next();
                run_benchmarks(target_id.as_deref());
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
        "Usage: cargo bench -p hivegui --bench local_runtime [--help|-h] [--manifest] [--targets] [--target <id>] [--baseline-path <id>]\n",
        "\n",
        "  --help|-h         Show this help text\n",
        "  --manifest        Print the shared benchmark manifest (default)\n",
        "  --targets         Print supported benchmark target identifiers\n",
        "  --target <id>     Print target metadata by id\n",
        "  --baseline-path <id>\n",
        "                   Print expected baseline path for a given target\n"
    );
    println!("{}", message);
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
        })).collect::<Vec<_>>(),
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&manifest).expect("serialize benchmark manifest")
    );
    eprintln!(
        "No placeholder timing was recorded: T093, T103, and T122 own the first approved Green runs."
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

fn target_by_id(id: &str) -> Option<TargetSpec> {
    target_specs()
        .iter()
        .find(|target| target.id == id)
        .cloned()
}

/// Execute one or more benchmark targets and print each report. The
/// process exits non-zero if any executed target fails its absolute
/// p95 budget. Baseline comparison and approval are review actions and
/// are intentionally not performed by the harness itself.
fn run_benchmarks(target_id: Option<&str>) {
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
                if !report.meets_absolute_budget() {
                    eprintln!(
                        "FAIL {}: p95={}ns exceeds budget {}ns",
                        target.id, report.percentiles_ns.p95, target.p95_budget_ns
                    );
                    failed = true;
                    continue;
                }
                eprintln!(
                    "PASS {}: p50={}ns p95={}ns p99={}ns (budget p95<={}ns)",
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
                let baseline = read_baseline(&baseline_path(&target, &environment));
                let comparison = compare_to_baseline(
                    &report,
                    baseline.as_ref(),
                    None,
                    chrono::Utc::now().date_naive(),
                );
                match comparison {
                    Ok(result) => match result.outcome {
                        ComparisonOutcome::Passed => {
                            eprintln!("  baseline: passed");
                        }
                        ComparisonOutcome::PendingBaseline => {
                            eprintln!("  baseline: pending (first approved Green establishes it)");
                        }
                        ComparisonOutcome::Blocked => {
                            eprintln!(
                                "  baseline: BLOCKED ({} regression(s) >10%)",
                                result.regressions.len()
                            );
                            failed = true;
                        }
                        ComparisonOutcome::ApprovedException => {
                            eprintln!("  baseline: approved exception");
                        }
                    },
                    Err(error) => {
                        eprintln!("  baseline: incompatible ({error})");
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
) -> Result<Option<BenchmarkReport>, MeasurementError> {
    match target.id.as_str() {
        TOOL_DISPATCH_ID => {
            let adapter = LocalToolAdapter::default_in_memory();
            measure_local_async(target.clone(), environment.clone(), move || {
                let adapter = adapter.clone();
                async move {
                    let request = ToolCallRequest::new("noop", ToolKind::Local, Value::Null);
                    // Excludes "user Function or Plugin execution": the
                    // in-memory dispatcher is a no-op terminal result.
                    let _ = adapter.dispatch(request).await;
                }
            })
            .await
            .map(Some)
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
        }
        _ => Ok(None),
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

/// Read a versioned baseline file, returning `None` when the file is
/// absent (a brand-new entry) or malformed.
fn read_baseline(path: &std::path::Path) -> Option<BenchmarkBaseline> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
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
