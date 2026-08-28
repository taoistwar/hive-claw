//! Shared benchmark schema, measurement helpers, and regression comparison.
//!
//! This module intentionally does not create or update baseline files. A new
//! target becomes eligible for a baseline only after its owning task has an
//! approved Green result; baseline creation is then an explicit review action.

use std::{
    fmt, fs,
    future::Future,
    hint::black_box,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use chrono::NaiveDate;
use hivegui::agent::local_agent::{
    LocalAgentActionScheduler, LocalAgentError, LocalAgentRuntime, LocalAgentScheduleFuture,
    ScheduledAgentAction, SessionHandle,
};
use hivegui::datasource::{
    conversation_store::{ConversationStore, ExecutionState},
    entity_store::{AgentInput, AgentStore},
    function_store::{FunctionInput, FunctionKind, FunctionStore},
    store::{Store, StoreOpenOptions},
    tool_store::{ToolInput, ToolKind, ToolSource, ToolStore},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const BASELINE_SCHEMA_VERSION: u32 = 1;
pub const REGRESSION_EXCEPTION_SCHEMA_VERSION: u32 = 1;
pub const FIXTURE_VERSION: &str = "hivegui-local-runtime-v1";
pub const BASELINE_ROOT: &str = "crates/hivegui/benches/baselines/v1";
pub const MEASURED_SAMPLES: usize = 100;
pub const WARMUP_ITERATIONS: usize = 10;
pub const REGRESSION_LIMIT_PERCENT: u64 = 10;

pub const AGENT_ACTION_DISPATCH_ID: &str = "agent_action_dispatch";
pub const AGENT_CRUD_ID: &str = "agent_crud";
pub const AGENT_SEARCH_PAGE_ID: &str = "agent_search_page";
pub const AGENT_FIXTURE_ROWS: usize = 125;
pub const AGENT_SEARCH_OPERATIONS_PER_SAMPLE: usize = 16;
pub const TOOL_DISPATCH_ID: &str = "tool_dispatch_batched_v2";
pub const TOOL_DISPATCH_OPERATIONS_PER_SAMPLE: usize = 1024;
pub const WORKFLOW_100_NODE_ID: &str = "workflow_100_node_noop";
pub const FUNCTION_CRUD_ID: &str = "function_crud";
pub const FUNCTION_SEARCH_PAGE_ID: &str = "function_search_page_pair_v2";
pub const FUNCTION_FIXTURE_ROWS: usize = 10_000;
pub const TOOL_CRUD_ID: &str = "tool_crud";
pub const TOOL_SEARCH_PAGE_ID: &str = "tool_search_page_pair_v2";
pub const TOOL_FIXTURE_ROWS: usize = 10_000;
pub const CONVERSATION_LIST_ID: &str = "conversation_list_recent_batched_v2";
pub const CONVERSATION_LIST_OPERATIONS_PER_SAMPLE: usize = 16;
pub const CONVERSATION_BUNDLE_ID: &str = "conversation_session_bundle_batched_v2";
pub const CONVERSATION_BUNDLE_OPERATIONS_PER_SAMPLE: usize = 16;
pub const CONVERSATION_CLEANUP_ID: &str = "conversation_retention_cleanup";
pub const CONVERSATION_RECOVERY_ID: &str = "conversation_running_recovery";
pub const CONVERSATION_FIXTURE_ROWS: usize = 125;
pub const RELEASE_MATRIX_SCHEMA_VERSION: u32 = 1;
pub const RELEASE_MATRIX_ID: &str = "hivegui_local_runtime_release_matrix_v1";
const RELEASE_MATRIX_AFFINITY: &str = "inherited/uncontrolled";

/// Reviewed interference-minimizing order for the final release matrix. This
/// reduces preceding heavy-load interference while keeping the result
/// falsifiable; it does not claim that host affinity is controlled.
pub const fn release_matrix_target_ids() -> [&'static str; 13] {
    [
        AGENT_ACTION_DISPATCH_ID,
        TOOL_DISPATCH_ID,
        CONVERSATION_LIST_ID,
        WORKFLOW_100_NODE_ID,
        CONVERSATION_BUNDLE_ID,
        AGENT_SEARCH_PAGE_ID,
        CONVERSATION_RECOVERY_ID,
        CONVERSATION_CLEANUP_ID,
        FUNCTION_CRUD_ID,
        TOOL_CRUD_ID,
        AGENT_CRUD_ID,
        FUNCTION_SEARCH_PAGE_ID,
        TOOL_SEARCH_PAGE_ID,
    ]
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMatrixInvocation {
    pub executable: PathBuf,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMatrixTargetResult {
    pub ordinal: usize,
    pub target_id: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMatrixReport {
    pub schema_version: u32,
    pub id: String,
    pub source_revision: String,
    pub affinity: String,
    pub source_stable: bool,
    pub status: String,
    pub targets: Vec<ReleaseMatrixTargetResult>,
}

pub fn release_matrix_invocations(
    executable: &Path,
    expected_source: &str,
) -> Vec<ReleaseMatrixInvocation> {
    release_matrix_target_ids()
        .into_iter()
        .map(|target_id| ReleaseMatrixInvocation {
            executable: executable.to_path_buf(),
            args: vec![
                "--run".into(),
                target_id.into(),
                "--expected-source".into(),
                expected_source.into(),
            ],
        })
        .collect()
}

pub fn run_release_matrix_with<SourceProbe, Launcher>(
    executable: &Path,
    expected_source: &str,
    mut source_probe: SourceProbe,
    mut launcher: Launcher,
) -> ReleaseMatrixReport
where
    SourceProbe: FnMut() -> String,
    Launcher: FnMut(&Path, &[String]) -> Result<i32, String>,
{
    let start_source = source_probe();
    let mut targets = Vec::with_capacity(release_matrix_target_ids().len());
    for (index, invocation) in release_matrix_invocations(executable, expected_source)
        .into_iter()
        .enumerate()
    {
        let (status, exit_code, error) = match launcher(&invocation.executable, &invocation.args) {
            Ok(0) => ("passed", Some(0), None),
            Ok(code) => ("failed", Some(code), None),
            Err(error) => ("failed", None, Some(error)),
        };
        targets.push(ReleaseMatrixTargetResult {
            ordinal: index + 1,
            target_id: release_matrix_target_ids()[index].into(),
            status: status.into(),
            exit_code,
            error,
        });
    }
    let end_source = source_probe();
    let source_stable = start_source == expected_source && end_source == expected_source;
    let status = if source_stable && targets.iter().all(|target| target.status == "passed") {
        "passed"
    } else {
        "failed"
    };

    ReleaseMatrixReport {
        schema_version: RELEASE_MATRIX_SCHEMA_VERSION,
        id: RELEASE_MATRIX_ID.into(),
        source_revision: expected_source.into(),
        affinity: RELEASE_MATRIX_AFFINITY.into(),
        source_stable,
        status: status.into(),
        targets,
    }
}

impl ReleaseMatrixReport {
    pub fn failed_setup(source_revision: &str, error: impl Into<String>) -> Self {
        let error = error.into();
        Self {
            schema_version: RELEASE_MATRIX_SCHEMA_VERSION,
            id: RELEASE_MATRIX_ID.into(),
            source_revision: source_revision.into(),
            affinity: RELEASE_MATRIX_AFFINITY.into(),
            source_stable: false,
            status: "failed".into(),
            targets: release_matrix_target_ids()
                .into_iter()
                .enumerate()
                .map(|(index, target_id)| ReleaseMatrixTargetResult {
                    ordinal: index + 1,
                    target_id: target_id.into(),
                    status: "failed".into(),
                    exit_code: None,
                    error: Some(error.clone()),
                })
                .collect(),
        }
    }
}

/// Fixed, deterministic 1-based page schedule used by the Function search/page
/// benchmark. Setup and fixture loading happen before timing starts.
pub const FUNCTION_SEARCH_PAGE_SCHEDULE: [FunctionSearchPageStep; MEASURED_SAMPLES] =
    function_page_schedule();
pub const AGENT_SEARCH_PAGE_SCHEDULE: [AgentSearchPageStep; MEASURED_SAMPLES] =
    agent_page_schedule();
pub const TOOL_SEARCH_PAGE_SCHEDULE: [ToolSearchPageStep; MEASURED_SAMPLES] = tool_page_schedule();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentSearchPageStep {
    pub search: Option<&'static str>,
    pub page: i64,
}

const fn agent_page_schedule() -> [AgentSearchPageStep; MEASURED_SAMPLES] {
    let mut schedule = [AgentSearchPageStep {
        search: None,
        page: 1,
    }; MEASURED_SAMPLES];
    let mut index = 0;
    while index < MEASURED_SAMPLES {
        let last_fixture_page = AGENT_FIXTURE_ROWS.div_ceil(20);
        schedule[index] = AgentSearchPageStep {
            search: if index % 2 == 0 {
                Some("fixture")
            } else {
                None
            },
            page: ((index * (last_fixture_page - 1) / (MEASURED_SAMPLES - 1)) + 1) as i64,
        };
        index += 1;
    }
    schedule
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionSearchPageStep {
    pub page: i64,
}

impl FunctionSearchPageStep {
    pub const fn routes(self) -> [(Option<&'static str>, i64); 2] {
        [(Some("fixture"), self.page), (None, self.page)]
    }
}

const fn function_page_schedule() -> [FunctionSearchPageStep; MEASURED_SAMPLES] {
    let mut schedule = [FunctionSearchPageStep { page: 1 }; MEASURED_SAMPLES];
    let mut index = 0;
    while index < MEASURED_SAMPLES {
        let last_fixture_page = FUNCTION_FIXTURE_ROWS / 20;
        schedule[index] = FunctionSearchPageStep {
            page: ((index * (last_fixture_page - 1) / (MEASURED_SAMPLES - 1)) + 1) as i64,
        };
        index += 1;
    }
    schedule
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolSearchPageStep {
    pub page: i64,
}

impl ToolSearchPageStep {
    pub const fn routes(self) -> [(Option<&'static str>, i64); 2] {
        [(Some("fixture"), self.page), (None, self.page)]
    }
}

const fn tool_page_schedule() -> [ToolSearchPageStep; MEASURED_SAMPLES] {
    let mut schedule = [ToolSearchPageStep { page: 1 }; MEASURED_SAMPLES];
    let mut index = 0;
    while index < MEASURED_SAMPLES {
        let last_fixture_page = TOOL_FIXTURE_ROWS / 20;
        schedule[index] = ToolSearchPageStep {
            page: ((index * (last_fixture_page - 1) / (MEASURED_SAMPLES - 1)) + 1) as i64,
        };
        index += 1;
    }
    schedule
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetSpec {
    pub id: String,
    pub owner_task: String,
    pub timing_boundary: String,
    pub excluded_time: Vec<String>,
    pub warmup_iterations: usize,
    pub measured_samples: usize,
    pub p95_budget_ns: u64,
    pub workflow_node_count: Option<usize>,
}

pub fn target_specs() -> [TargetSpec; 13] {
    [
        TargetSpec {
            id: AGENT_ACTION_DISPATCH_ID.into(),
            owner_task: "T122".into(),
            timing_boundary: "parsed LLM decision to next local action scheduled".into(),
            excluded_time: vec![
                "external LLM latency".into(),
                "network latency".into(),
                "user Function or Plugin execution".into(),
            ],
            warmup_iterations: WARMUP_ITERATIONS,
            measured_samples: MEASURED_SAMPLES,
            p95_budget_ns: 200_000_000,
            workflow_node_count: None,
        },
        TargetSpec {
            id: AGENT_CRUD_ID.into(),
            owner_task: "T115".into(),
            timing_boundary: "Agent CRUD lifecycle accepted through validation, create/fetch/update/delete transactions, and final absence materialized".into(),
            excluded_time: vec![
                "external I/O".into(),
                "Agent execution".into(),
                "Store open, migration, fixture loading, and baseline I/O".into(),
            ],
            warmup_iterations: WARMUP_ITERATIONS,
            measured_samples: MEASURED_SAMPLES,
            p95_budget_ns: 1_000_000_000,
            workflow_node_count: None,
        },
        TargetSpec {
            id: AGENT_SEARCH_PAGE_ID.into(),
            owner_task: "T115".into(),
            timing_boundary: "Agent search/page request accepted through normalization to one fixed 20-row page and total metadata materialized".into(),
            excluded_time: vec![
                "external I/O".into(),
                "Agent execution".into(),
                "Store open, migration, 125-row fixture loading, and baseline I/O".into(),
            ],
            warmup_iterations: WARMUP_ITERATIONS,
            measured_samples: MEASURED_SAMPLES,
            p95_budget_ns: 500_000_000,
            workflow_node_count: None,
        },
        TargetSpec {
            id: TOOL_DISPATCH_ID.into(),
            owner_task: "T103".into(),
            timing_boundary: "one per-dispatch normalized sample from a fixed batch of 1024 identical persisted Function-wrap Tool requests, each measured from persisted Tool execution acceptance through Tool/Function target lookup, input-schema and Capability validation, local no-op target return, output-schema validation, and validated result materialization".into(),
            excluded_time: vec![
                "network latency".into(),
                "user Function or Plugin execution".into(),
            ],
            warmup_iterations: WARMUP_ITERATIONS,
            measured_samples: MEASURED_SAMPLES,
            p95_budget_ns: 50_000_000,
            workflow_node_count: None,
        },
        TargetSpec {
            id: WORKFLOW_100_NODE_ID.into(),
            owner_task: "T093".into(),
            timing_boundary: "validated 100-node no-op DAG ready to all nodes scheduled".into(),
            excluded_time: vec!["user Function or Plugin execution".into()],
            warmup_iterations: WARMUP_ITERATIONS,
            measured_samples: MEASURED_SAMPLES,
            p95_budget_ns: 100_000_000,
            workflow_node_count: Some(100),
        },
        TargetSpec {
            id: FUNCTION_CRUD_ID.into(),
            owner_task: "T083".into(),
            timing_boundary: "Function CRUD lifecycle accepted through validation, create/get/update/delete transactions, and final absence materialized".into(),
            excluded_time: vec![
                "external I/O".into(),
                "Plugin/WASM execution".into(),
                "Store open, migration, fixture loading, and baseline I/O".into(),
            ],
            warmup_iterations: WARMUP_ITERATIONS,
            measured_samples: MEASURED_SAMPLES,
            p95_budget_ns: 1_000_000_000,
            workflow_node_count: None,
        },
        TargetSpec {
            id: FUNCTION_SEARCH_PAGE_ID.into(),
            owner_task: "T083".into(),
            timing_boundary: "one combined wall-clock sample covering filtered then unfiltered Function search/page requests at the same page depth, from the filtered request accepted through both fixed 20-row pages and total-order metadata materialized, without per-request normalization".into(),
            excluded_time: vec![
                "external I/O".into(),
                "Plugin/WASM execution".into(),
                "Store open, migration, 10,000-row fixture loading, and baseline I/O".into(),
            ],
            warmup_iterations: WARMUP_ITERATIONS,
            measured_samples: MEASURED_SAMPLES,
            p95_budget_ns: 500_000_000,
            workflow_node_count: None,
        },
        TargetSpec {
            id: TOOL_CRUD_ID.into(),
            owner_task: "T101".into(),
            timing_boundary: "Tool CRUD lifecycle accepted through validation, create/get/update/delete transactions, and final absence materialized".into(),
            excluded_time: vec![
                "external I/O".into(),
                "Function, Workflow, Plugin, and Agent execution".into(),
                "Store open, migration, fixture loading, and baseline I/O".into(),
            ],
            warmup_iterations: WARMUP_ITERATIONS,
            measured_samples: MEASURED_SAMPLES,
            p95_budget_ns: 1_000_000_000,
            workflow_node_count: None,
        },
        TargetSpec {
            id: TOOL_SEARCH_PAGE_ID.into(),
            owner_task: "T101".into(),
            timing_boundary: "one combined wall-clock sample covering filtered then unfiltered Tool search/page requests at the same page depth, from the filtered request accepted through both fixed 20-row pages and total-order metadata materialized, without per-request normalization".into(),
            excluded_time: vec![
                "external I/O".into(),
                "Function, Workflow, Plugin, and Agent execution".into(),
                "Store open, migration, 10,000-row fixture loading, and baseline I/O".into(),
            ],
            warmup_iterations: WARMUP_ITERATIONS,
            measured_samples: MEASURED_SAMPLES,
            p95_budget_ns: 500_000_000,
            workflow_node_count: None,
        },
        TargetSpec {
            id: CONVERSATION_LIST_ID.into(),
            owner_task: "T117".into(),
            timing_boundary: "one per-request normalized sample from a fixed batch of 16 recent Conversation requests against the same fixed 125-session fixture, each request using its own current UTC upper bound and accepted through one indexed 20-row encrypted-session page and exact newest-first total order materialized".into(),
            excluded_time: vec![
                "external I/O".into(),
                "Store open, migration, 125-row fixture loading, and baseline I/O".into(),
            ],
            warmup_iterations: WARMUP_ITERATIONS,
            measured_samples: MEASURED_SAMPLES,
            p95_budget_ns: 1_000_000_000,
            workflow_node_count: None,
        },
        TargetSpec {
            id: CONVERSATION_BUNDLE_ID.into(),
            owner_task: "T117".into(),
            timing_boundary: "one per-request normalized sample from a fixed batch of 16 identical 25-session Conversation bundle requests, each request accepted through exactly two encrypted child queries with one message and one execution per session materialized".into(),
            excluded_time: vec![
                "external I/O".into(),
                "Store open, migration, fixture loading, and baseline I/O".into(),
            ],
            warmup_iterations: WARMUP_ITERATIONS,
            measured_samples: MEASURED_SAMPLES,
            p95_budget_ns: 1_000_000_000,
            workflow_node_count: None,
        },
        TargetSpec {
            id: CONVERSATION_CLEANUP_ID.into(),
            owner_task: "T117".into(),
            timing_boundary: "one encrypted session create, expiry transition, exact retention preview, and confirmed cascade deletion lifecycle".into(),
            excluded_time: vec![
                "external I/O".into(),
                "Store open, migration, and baseline I/O".into(),
            ],
            warmup_iterations: WARMUP_ITERATIONS,
            measured_samples: MEASURED_SAMPLES,
            p95_budget_ns: 1_000_000_000,
            workflow_node_count: None,
        },
        TargetSpec {
            id: CONVERSATION_RECOVERY_ID.into(),
            owner_task: "T117".into(),
            timing_boundary: "one encrypted running execution create, interrupted recovery, and idempotent replay lifecycle".into(),
            excluded_time: vec![
                "external I/O".into(),
                "Store open, migration, and baseline I/O".into(),
            ],
            warmup_iterations: WARMUP_ITERATIONS,
            measured_samples: MEASURED_SAMPLES,
            p95_budget_ns: 1_000_000_000,
            workflow_node_count: None,
        },
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentFingerprint {
    pub os: String,
    pub architecture: String,
    pub rustc: String,
    pub build_profile: String,
    pub cpu_model: String,
    pub logical_cpus: usize,
}

impl EnvironmentFingerprint {
    pub fn capture() -> Self {
        Self {
            os: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            rustc: command_output("rustc", &["--version"])
                .unwrap_or_else(|| "rustc unavailable".into()),
            build_profile: if cfg!(debug_assertions) {
                "debug".into()
            } else {
                "release".into()
            },
            cpu_model: cpu_model(),
            logical_cpus: std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1),
        }
    }

    pub fn baseline_key(&self) -> String {
        let logical_cpus = self.logical_cpus.to_string();
        [
            self.os.as_str(),
            self.architecture.as_str(),
            self.rustc.as_str(),
            self.build_profile.as_str(),
            self.cpu_model.as_str(),
            logical_cpus.as_str(),
        ]
        .into_iter()
        .map(slug)
        .collect::<Vec<_>>()
        .join("--")
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PercentilesNs {
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
}

impl PercentilesNs {
    pub fn from_samples(samples_ns: &[u64]) -> Result<Self, MeasurementError> {
        if samples_ns.is_empty() {
            return Err(MeasurementError::NoSamples);
        }
        let mut sorted = samples_ns.to_vec();
        sorted.sort_unstable();
        Ok(Self {
            p50: nearest_rank(&sorted, 50),
            p95: nearest_rank(&sorted, 95),
            p99: nearest_rank(&sorted, 99),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkReport {
    pub schema_version: u32,
    pub fixture_version: String,
    pub target: TargetSpec,
    pub environment: EnvironmentFingerprint,
    pub sample_count: usize,
    pub percentiles_ns: PercentilesNs,
}

impl BenchmarkReport {
    pub fn from_samples(
        target: TargetSpec,
        environment: EnvironmentFingerprint,
        samples_ns: &[u64],
    ) -> Result<Self, MeasurementError> {
        if samples_ns.len() != target.measured_samples {
            return Err(MeasurementError::WrongSampleCount {
                expected: target.measured_samples,
                actual: samples_ns.len(),
            });
        }
        Ok(Self {
            schema_version: BASELINE_SCHEMA_VERSION,
            fixture_version: FIXTURE_VERSION.into(),
            target,
            environment,
            sample_count: samples_ns.len(),
            percentiles_ns: PercentilesNs::from_samples(samples_ns)?,
        })
    }

    pub fn meets_absolute_budget(&self) -> bool {
        self.percentiles_ns.p95 <= self.target.p95_budget_ns
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BaselineApproval {
    pub reviewer: String,
    pub approved_at: String,
    pub source_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkBaseline {
    pub report: BenchmarkReport,
    pub approval: BaselineApproval,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselineLoadError {
    Read { path: PathBuf, message: String },
    Decode { path: PathBuf, message: String },
}

impl fmt::Display for BaselineLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, message } => {
                write!(
                    formatter,
                    "cannot read baseline {}: {message}",
                    path.display()
                )
            }
            Self::Decode { path, message } => write!(
                formatter,
                "cannot decode baseline {}: {message}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for BaselineLoadError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegressionExceptionLoadError {
    Read { path: PathBuf, message: String },
    Decode { path: PathBuf, message: String },
}

impl fmt::Display for RegressionExceptionLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, message } => write!(
                formatter,
                "cannot read regression exception {}: {message}",
                path.display()
            ),
            Self::Decode { path, message } => write!(
                formatter,
                "cannot decode regression exception {}: {message}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RegressionExceptionLoadError {}

#[derive(Debug)]
pub enum BenchmarkGateError {
    Baseline(BaselineLoadError),
    RegressionException(RegressionExceptionLoadError),
    Compatibility(CompatibilityError),
}

impl fmt::Display for BenchmarkGateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Baseline(error) => error.fmt(formatter),
            Self::RegressionException(error) => error.fmt(formatter),
            Self::Compatibility(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for BenchmarkGateError {}

impl From<BaselineLoadError> for BenchmarkGateError {
    fn from(error: BaselineLoadError) -> Self {
        Self::Baseline(error)
    }
}

impl From<RegressionExceptionLoadError> for BenchmarkGateError {
    fn from(error: RegressionExceptionLoadError) -> Self {
        Self::RegressionException(error)
    }
}

impl From<CompatibilityError> for BenchmarkGateError {
    fn from(error: CompatibilityError) -> Self {
        Self::Compatibility(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RegressionException {
    pub schema_version: u32,
    pub target: TargetSpec,
    pub environment: EnvironmentFingerprint,
    pub current_source_revision: String,
    pub baseline: RegressionExceptionBaseline,
    pub approved_regressions: Vec<ApprovedRegression>,
    pub approval: RegressionExceptionApproval,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RegressionExceptionBaseline {
    pub approval: BaselineApproval,
    pub percentiles_ns: PercentilesNs,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RegressionExceptionApproval {
    pub signer: String,
    pub approved_at: String,
    pub reason: String,
    pub impact_scope: String,
    pub review_due: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Percentile {
    P50,
    P95,
    P99,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ApprovedRegression {
    pub percentile: Percentile,
    pub baseline_ns: u64,
    pub observed_current_ns: u64,
    pub approved_max_current_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Regression {
    pub percentile: Percentile,
    pub baseline_ns: u64,
    pub current_ns: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonOutcome {
    Passed,
    PendingBaseline,
    Blocked,
    ApprovedException,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ComparisonResult {
    pub outcome: ComparisonOutcome,
    pub regressions: Vec<Regression>,
    pub exception_rejection: Option<String>,
}

pub fn compare_to_baseline(
    current: &BenchmarkReport,
    baseline: Option<&BenchmarkBaseline>,
    exception: Option<&RegressionException>,
    _as_of: NaiveDate,
) -> Result<ComparisonResult, CompatibilityError> {
    let Some(baseline) = baseline else {
        return Ok(ComparisonResult {
            outcome: ComparisonOutcome::PendingBaseline,
            regressions: Vec::new(),
            exception_rejection: None,
        });
    };

    validate_compatibility(current, baseline)?;
    let regressions = regressions(baseline.report.percentiles_ns, current.percentiles_ns);
    if regressions.is_empty() {
        return Ok(ComparisonResult {
            outcome: ComparisonOutcome::Passed,
            regressions,
            exception_rejection: None,
        });
    }

    match exception {
        Some(_) => Ok(ComparisonResult {
            outcome: ComparisonOutcome::Blocked,
            regressions,
            exception_rejection: Some(
                "context-bound regression exceptions require evaluate_benchmark_gate".into(),
            ),
        }),
        None => Ok(ComparisonResult {
            outcome: ComparisonOutcome::Blocked,
            regressions,
            exception_rejection: None,
        }),
    }
}

pub fn baseline_path(target: &TargetSpec, environment: &EnvironmentFingerprint) -> PathBuf {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("hivegui manifest directory must be nested under the workspace root");

    workspace_root
        .join(BASELINE_ROOT)
        .join(&target.id)
        .join(format!("{}.json", environment.baseline_key()))
}

pub fn regression_exception_path(
    target: &TargetSpec,
    environment: &EnvironmentFingerprint,
) -> PathBuf {
    baseline_path(target, environment).with_extension("exception.json")
}

/// Capture the exact benchmark-affecting source state without reading from or
/// mutating Git's index. The returned value binds the commit identity to a
/// deterministic digest of tracked and non-ignored untracked worktree entries.
pub fn source_revision(repository: &Path) -> Result<String, SourceRevisionError> {
    let root_output = git_output(repository, &["rev-parse", "--show-toplevel"])?;
    let root_text = std::str::from_utf8(&root_output)
        .map_err(|error| SourceRevisionError::InvalidGitOutput(error.to_string()))?;
    let root = PathBuf::from(root_text.trim_end_matches(['\r', '\n']));
    if root.as_os_str().is_empty() {
        return Err(SourceRevisionError::InvalidGitOutput(
            "git returned an empty repository root".into(),
        ));
    }

    let head_output = git_output(&root, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let head = std::str::from_utf8(&head_output)
        .map_err(|error| SourceRevisionError::InvalidGitOutput(error.to_string()))?
        .trim_end_matches(['\r', '\n']);
    if head.len() != 40 || !head.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SourceRevisionError::InvalidGitOutput(format!(
            "git returned an invalid HEAD object id: {head:?}"
        )));
    }

    let listed = git_output(
        &root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "--full-name",
            "-z",
            "--",
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            ".cargo",
            "crates",
            "third_party",
        ],
    )?;
    let mut paths = listed
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .filter(|path| !is_excluded_source_path(path))
        .map(<[u8]>::to_vec)
        .collect::<Vec<_>>();
    paths.sort_unstable();
    paths.dedup();

    let mut digest = Sha256::new();
    hash_field(&mut digest, b"hivegui-source-v1");
    hash_field(&mut digest, head.as_bytes());
    for path_bytes in paths {
        let path_text = std::str::from_utf8(&path_bytes).map_err(|error| {
            SourceRevisionError::InvalidGitOutput(format!(
                "source path is not valid UTF-8: {error}"
            ))
        })?;
        let relative = Path::new(path_text);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(SourceRevisionError::InvalidGitOutput(format!(
                "git returned unsafe source path {path_text:?}"
            )));
        }
        let absolute = root.join(relative);
        let (state, executable, contents) = match fs::symlink_metadata(&absolute) {
            Ok(metadata) if metadata.file_type().is_file() => (
                b"file".as_slice(),
                executable_bit(&metadata),
                fs::read(&absolute).map_err(|error| SourceRevisionError::Entry {
                    path: relative.to_path_buf(),
                    message: error.to_string(),
                })?,
            ),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let target =
                    fs::read_link(&absolute).map_err(|error| SourceRevisionError::Entry {
                        path: relative.to_path_buf(),
                        message: error.to_string(),
                    })?;
                (
                    b"symlink".as_slice(),
                    false,
                    target.as_os_str().as_encoded_bytes().to_vec(),
                )
            }
            Ok(_) => {
                return Err(SourceRevisionError::Entry {
                    path: relative.to_path_buf(),
                    message: "source entry is neither a regular file nor a symlink".into(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (b"deleted".as_slice(), false, Vec::new())
            }
            Err(error) => {
                return Err(SourceRevisionError::Entry {
                    path: relative.to_path_buf(),
                    message: error.to_string(),
                });
            }
        };
        hash_field(&mut digest, &path_bytes);
        hash_field(&mut digest, state);
        hash_field(&mut digest, &[u8::from(executable)]);
        hash_field(&mut digest, &contents);
    }

    Ok(format!(
        "git:{head}+hivegui-source-v1:{:x}",
        digest.finalize()
    ))
}

fn is_excluded_source_path(path: &[u8]) -> bool {
    const BASELINE_DIRECTORY: &[u8] = b"crates/hivegui/benches/baselines";
    path == BASELINE_DIRECTORY
        || path
            .strip_prefix(BASELINE_DIRECTORY)
            .is_some_and(|suffix| suffix.starts_with(b"/"))
}

fn hash_field(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

#[cfg(unix)]
fn executable_bit(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable_bit(_metadata: &fs::Metadata) -> bool {
    false
}

fn git_output(repository: &Path, arguments: &[&str]) -> Result<Vec<u8>, SourceRevisionError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .map_err(|error| SourceRevisionError::Git {
            arguments: arguments.join(" "),
            message: error.to_string(),
        })?;
    if !output.status.success() {
        return Err(SourceRevisionError::Git {
            arguments: arguments.join(" "),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(output.stdout)
}

#[derive(Debug)]
pub enum SourceRevisionError {
    Git { arguments: String, message: String },
    InvalidGitOutput(String),
    Entry { path: PathBuf, message: String },
}

impl fmt::Display for SourceRevisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git { arguments, message } => {
                write!(formatter, "git {arguments} failed: {message}")
            }
            Self::InvalidGitOutput(message) => {
                write!(formatter, "cannot capture source revision: {message}")
            }
            Self::Entry { path, message } => {
                write!(
                    formatter,
                    "cannot fingerprint {}: {message}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for SourceRevisionError {}

/// Load an approved baseline without conflating absence with corruption.
/// Only `NotFound` represents a brand-new target eligible for
/// `PendingBaseline`; every other read or decode failure is release-blocking.
pub fn load_baseline(path: &Path) -> Result<Option<BenchmarkBaseline>, BaselineLoadError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(BaselineLoadError::Read {
                path: path.to_path_buf(),
                message: "baseline path is not a regular file".to_string(),
            });
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(BaselineLoadError::Read {
                path: path.to_path_buf(),
                message: error.to_string(),
            });
        }
    }
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            return Err(BaselineLoadError::Read {
                path: path.to_path_buf(),
                message: error.to_string(),
            });
        }
    };
    serde_json::from_str(&content)
        .map(Some)
        .map_err(|error| BaselineLoadError::Decode {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
}

/// Load one explicitly reviewed regression-exception sidecar. Only a truly
/// absent path means that no exception was approved; every existing malformed,
/// redirected, or non-file entry fails closed.
pub fn load_regression_exception(
    path: &Path,
) -> Result<Option<RegressionException>, RegressionExceptionLoadError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(RegressionExceptionLoadError::Read {
                path: path.to_path_buf(),
                message: "regression exception path is not a regular file".to_string(),
            });
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(RegressionExceptionLoadError::Read {
                path: path.to_path_buf(),
                message: error.to_string(),
            });
        }
    }
    let content = fs::read_to_string(path).map_err(|error| RegressionExceptionLoadError::Read {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    serde_json::from_str(&content)
        .map(Some)
        .map_err(|error| RegressionExceptionLoadError::Decode {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
}

/// Evaluate the complete release performance gate using canonical, read-only
/// baseline and exception artifacts. Relative-regression exceptions are bound
/// to the exact report context and never waive the absolute p95 budget.
pub fn evaluate_benchmark_gate(
    current: &BenchmarkReport,
    current_source_revision: &str,
    baseline_path: &Path,
    exception_path: &Path,
    as_of: NaiveDate,
) -> Result<ComparisonResult, BenchmarkGateError> {
    let baseline = load_baseline(baseline_path)?;
    let exception = load_regression_exception(exception_path)?;
    let Some(baseline) = baseline.as_ref() else {
        if !current.meets_absolute_budget() {
            return Ok(ComparisonResult {
                outcome: ComparisonOutcome::Blocked,
                regressions: Vec::new(),
                exception_rejection: Some(format!(
                    "absolute p95 budget exceeded: current={}ns budget={}ns",
                    current.percentiles_ns.p95, current.target.p95_budget_ns
                )),
            });
        }
        if exception.is_some() {
            return Ok(ComparisonResult {
                outcome: ComparisonOutcome::Blocked,
                regressions: Vec::new(),
                exception_rejection: Some(
                    "regression exception requires an approved baseline".into(),
                ),
            });
        }
        return Ok(ComparisonResult {
            outcome: ComparisonOutcome::PendingBaseline,
            regressions: Vec::new(),
            exception_rejection: None,
        });
    };

    validate_compatibility(current, baseline)?;
    let regressions = regressions(baseline.report.percentiles_ns, current.percentiles_ns);
    if !current.meets_absolute_budget() {
        return Ok(ComparisonResult {
            outcome: ComparisonOutcome::Blocked,
            regressions,
            exception_rejection: Some(format!(
                "absolute p95 budget exceeded: current={}ns budget={}ns",
                current.percentiles_ns.p95, current.target.p95_budget_ns
            )),
        });
    }
    if let Some(exception) = exception.as_ref()
        && let Err(reason) = validate_bound_exception(
            current,
            current_source_revision,
            baseline,
            &regressions,
            exception,
            as_of,
        )
    {
        return Ok(ComparisonResult {
            outcome: ComparisonOutcome::Blocked,
            regressions,
            exception_rejection: Some(reason),
        });
    }
    if regressions.is_empty() {
        return Ok(ComparisonResult {
            outcome: ComparisonOutcome::Passed,
            regressions,
            exception_rejection: None,
        });
    }

    let Some(_) = exception.as_ref() else {
        return Ok(ComparisonResult {
            outcome: ComparisonOutcome::Blocked,
            regressions,
            exception_rejection: None,
        });
    };
    Ok(ComparisonResult {
        outcome: ComparisonOutcome::ApprovedException,
        regressions,
        exception_rejection: None,
    })
}

pub fn measure_local<R>(
    target: TargetSpec,
    environment: EnvironmentFingerprint,
    mut operation: impl FnMut() -> R,
) -> Result<BenchmarkReport, MeasurementError> {
    for _ in 0..target.warmup_iterations {
        black_box(operation());
    }

    let mut samples = Vec::with_capacity(target.measured_samples);
    for _ in 0..target.measured_samples {
        let started = Instant::now();
        black_box(operation());
        samples.push(duration_ns(started)?);
    }
    BenchmarkReport::from_samples(target, environment, &samples)
}

pub async fn measure_local_async<F, Fut, R>(
    target: TargetSpec,
    environment: EnvironmentFingerprint,
    mut operation: F,
) -> Result<BenchmarkReport, MeasurementError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = R>,
{
    for _ in 0..target.warmup_iterations {
        black_box(operation().await);
    }

    let mut samples = Vec::with_capacity(target.measured_samples);
    for _ in 0..target.measured_samples {
        let started = Instant::now();
        black_box(operation().await);
        samples.push(duration_ns(started)?);
    }
    BenchmarkReport::from_samples(target, environment, &samples)
}

/// Measure a fast asynchronous boundary in fixed batches and normalize every
/// recorded duration back to one operation. Batching prevents scheduler and
/// clock noise from dominating sub-millisecond targets without changing their
/// per-operation percentile units.
pub async fn measure_local_async_batched<F, Fut, R>(
    target: TargetSpec,
    environment: EnvironmentFingerprint,
    operations_per_sample: usize,
    mut operation: F,
) -> Result<BenchmarkReport, MeasurementError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = R>,
{
    let divisor = u64::try_from(operations_per_sample)
        .ok()
        .filter(|divisor| *divisor > 0)
        .ok_or(MeasurementError::InvalidBatchSize)?;

    for _ in 0..target.warmup_iterations {
        for _ in 0..operations_per_sample {
            black_box(operation().await);
        }
    }

    let mut samples = Vec::with_capacity(target.measured_samples);
    for _ in 0..target.measured_samples {
        let started = Instant::now();
        for _ in 0..operations_per_sample {
            black_box(operation().await);
        }
        samples.push(duration_ns(started)? / divisor);
    }
    BenchmarkReport::from_samples(target, environment, &samples)
}

/// Execute a benchmark target whose fixture and operation are owned by this
/// shared harness. Unknown targets return `None` so the runtime benchmark can
/// retain its existing Tool and Workflow adapters.
pub async fn run_target(
    target: &TargetSpec,
    environment: &EnvironmentFingerprint,
) -> Result<Option<BenchmarkReport>, BenchmarkRunError> {
    match target.id.as_str() {
        AGENT_ACTION_DISPATCH_ID => run_agent_action_dispatch(target, environment)
            .await
            .map(Some),
        AGENT_CRUD_ID => run_agent_crud(target, environment).await.map(Some),
        AGENT_SEARCH_PAGE_ID => run_agent_search_page(target, environment).await.map(Some),
        FUNCTION_CRUD_ID => run_function_crud(target, environment).await.map(Some),
        FUNCTION_SEARCH_PAGE_ID => run_function_search_page_pair_v2(target, environment)
            .await
            .map(Some),
        TOOL_CRUD_ID => run_tool_crud(target, environment).await.map(Some),
        TOOL_SEARCH_PAGE_ID => run_tool_search_page_pair_v2(target, environment)
            .await
            .map(Some),
        CONVERSATION_LIST_ID => run_conversation_list(target, environment).await.map(Some),
        CONVERSATION_BUNDLE_ID => run_conversation_bundle(target, environment).await.map(Some),
        CONVERSATION_CLEANUP_ID => run_conversation_cleanup(target, environment)
            .await
            .map(Some),
        CONVERSATION_RECOVERY_ID => run_conversation_recovery(target, environment)
            .await
            .map(Some),
        _ => Ok(None),
    }
}

async fn run_agent_action_dispatch(
    target: &TargetSpec,
    environment: &EnvironmentFingerprint,
) -> Result<BenchmarkReport, BenchmarkRunError> {
    let fixture = AgentBenchmarkFixture::open().await?;
    let runtime = fixture.runtime.clone();
    let handle = fixture.handle.clone();
    let scheduler = std::sync::Arc::new(NoopAgentActionScheduler);
    let _fixture_lifetime = (fixture.store, fixture.temporary_root);

    measure_local_async_checked(target.clone(), environment.clone(), move || {
        let runtime = runtime.clone();
        let handle = handle.clone();
        let scheduler = std::sync::Arc::clone(&scheduler);
        async move {
            let action = runtime
                .schedule_decision(
                    &handle,
                    r#"{"action":"reply","content":"benchmark-local-reply"}"#,
                    scheduler.as_ref(),
                )
                .await
                .map_err(BenchmarkRunError::from_operation)?;
            if !matches!(action, ScheduledAgentAction::Reply { .. }) {
                return Err(BenchmarkRunError::Operation(
                    "Agent benchmark scheduled the wrong local action".into(),
                ));
            }
            Ok(())
        }
    })
    .await
}

struct AgentBenchmarkFixture {
    runtime: LocalAgentRuntime,
    handle: SessionHandle,
    store: Store,
    temporary_root: tempfile::TempDir,
}

impl AgentBenchmarkFixture {
    async fn open() -> Result<Self, BenchmarkRunError> {
        let temporary_root = tempfile::tempdir().map_err(BenchmarkRunError::from_setup)?;
        let store = Store::open_local(StoreOpenOptions::new(
            temporary_root.path().join("hivegui.db"),
            temporary_root.path().join("plugins"),
        ))
        .await
        .map_err(BenchmarkRunError::from_setup)?;
        let agents =
            AgentStore::new(store.pool().clone()).map_err(BenchmarkRunError::from_setup)?;
        agents
            .create(
                AgentInput::new_root(
                    "agent-action-benchmark",
                    "Agent action benchmark",
                    "Schedule only local actions.",
                )
                .map_err(BenchmarkRunError::from_setup)?,
            )
            .await
            .map_err(BenchmarkRunError::from_setup)?;
        let runtime =
            LocalAgentRuntime::new(store.pool().clone()).map_err(BenchmarkRunError::from_setup)?;
        let (handle, _cancel) = runtime
            .start_session("benchmark user message")
            .await
            .map_err(BenchmarkRunError::from_setup)?;
        Ok(Self {
            runtime,
            handle,
            store,
            temporary_root,
        })
    }
}

struct NoopAgentActionScheduler;

impl LocalAgentActionScheduler for NoopAgentActionScheduler {
    fn schedule(&self, _action: ScheduledAgentAction) -> LocalAgentScheduleFuture {
        Box::pin(async { Ok::<(), LocalAgentError>(()) })
    }
}

async fn run_agent_crud(
    target: &TargetSpec,
    environment: &EnvironmentFingerprint,
) -> Result<BenchmarkReport, BenchmarkRunError> {
    let fixture = AgentManagementBenchmarkFixture::open().await?;
    let store = fixture.agents.clone();
    let sequence = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let _fixture_lifetime = (fixture.store, fixture.temporary_root);

    measure_local_async_checked(target.clone(), environment.clone(), move || {
        let store = store.clone();
        let sequence = std::sync::Arc::clone(&sequence);
        async move {
            let sequence = sequence.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let identifier = format!("agent_benchmark_crud_{sequence:06}");
            let created = store
                .create(
                    AgentInput::new_root(
                        identifier.clone(),
                        format!("Agent benchmark CRUD {sequence:06}"),
                        "local benchmark prompt",
                    )
                    .map_err(BenchmarkRunError::from_operation)?,
                )
                .await
                .map_err(BenchmarkRunError::from_operation)?;
            let loaded = store
                .fetch_one(created.id())
                .await
                .map_err(BenchmarkRunError::from_operation)?
                .ok_or_else(|| {
                    BenchmarkRunError::Operation("created Agent was not materialized".into())
                })?;
            if loaded.identifier() != identifier {
                return Err(BenchmarkRunError::Operation(
                    "created Agent identifier changed during materialization".into(),
                ));
            }
            let updated = store
                .update(
                    created.id(),
                    AgentInput::new_root(
                        identifier,
                        format!("Agent benchmark CRUD updated {sequence:06}"),
                        "updated local benchmark prompt",
                    )
                    .map_err(BenchmarkRunError::from_operation)?,
                )
                .await
                .map_err(BenchmarkRunError::from_operation)?;
            store
                .delete(updated.id(), None)
                .await
                .map_err(BenchmarkRunError::from_operation)?;
            if store
                .fetch_one(updated.id())
                .await
                .map_err(BenchmarkRunError::from_operation)?
                .is_some()
            {
                return Err(BenchmarkRunError::Operation(
                    "deleted Agent remained materialized".into(),
                ));
            }
            Ok(())
        }
    })
    .await
}

async fn run_agent_search_page(
    target: &TargetSpec,
    environment: &EnvironmentFingerprint,
) -> Result<BenchmarkReport, BenchmarkRunError> {
    let fixture = AgentManagementBenchmarkFixture::open().await?;
    for index in 0..AGENT_FIXTURE_ROWS {
        fixture
            .agents
            .create(
                AgentInput::new_root(
                    format!("agent_fixture_{index:04}"),
                    format!("Agent fixture {index:04}"),
                    "local fixture prompt",
                )
                .map_err(BenchmarkRunError::from_setup)?,
            )
            .await
            .map_err(BenchmarkRunError::from_setup)?;
    }
    checkpoint_fixture_wal(&fixture.store, "Agent search fixture").await?;
    let store = fixture.agents.clone();
    let sequence = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let _fixture_lifetime = (fixture.store, fixture.temporary_root);

    measure_local_async_checked_batched(
        target.clone(),
        environment.clone(),
        AGENT_SEARCH_OPERATIONS_PER_SAMPLE,
        move || {
        let store = store.clone();
        let sequence = std::sync::Arc::clone(&sequence);
        async move {
            let index = (sequence.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                / AGENT_SEARCH_OPERATIONS_PER_SAMPLE)
                % AGENT_SEARCH_PAGE_SCHEDULE.len();
            let step = AGENT_SEARCH_PAGE_SCHEDULE[index];
            let mut filter = hivegui::datasource::entity_store::AgentFilter::first()
                .with_page(step.page);
            if let Some(search) = step.search {
                filter = filter.with_search(search);
            }
            let page = store
                .search(&filter)
                .await
                .map_err(BenchmarkRunError::from_operation)?;
            let expected_total = if step.search.is_some() {
                AGENT_FIXTURE_ROWS as i64
            } else {
                (AGENT_FIXTURE_ROWS + 1) as i64
            };
            if page.total() != expected_total || page.page() != step.page {
                return Err(BenchmarkRunError::Operation(format!(
                    "Agent search/page metadata drifted: total={} expected={} page={} expected_page={}",
                    page.total(),
                    expected_total,
                    page.page(),
                    step.page
                )));
            }
            let page_start = usize::try_from((step.page - 1) * 20).unwrap_or(usize::MAX);
            let expected_len = usize::min(
                20,
                usize::try_from(expected_total)
                    .unwrap_or_default()
                    .saturating_sub(page_start),
            );
            if page.records().len() != expected_len {
                return Err(BenchmarkRunError::Operation(format!(
                    "Agent search/page row count drifted: actual={} expected={expected_len}",
                    page.records().len()
                )));
            }
            Ok(())
        }
    },
    )
    .await
}

struct AgentManagementBenchmarkFixture {
    agents: AgentStore,
    store: Store,
    temporary_root: tempfile::TempDir,
}

impl AgentManagementBenchmarkFixture {
    async fn open() -> Result<Self, BenchmarkRunError> {
        let temporary_root = tempfile::tempdir().map_err(BenchmarkRunError::from_setup)?;
        let store = Store::open_local(StoreOpenOptions::new(
            temporary_root.path().join("hivegui.db"),
            temporary_root.path().join("plugins"),
        ))
        .await
        .map_err(BenchmarkRunError::from_setup)?;
        let agents =
            AgentStore::new(store.pool().clone()).map_err(BenchmarkRunError::from_setup)?;
        agents
            .create(
                AgentInput::new_root(
                    "agent_benchmark_default",
                    "Agent benchmark default",
                    "local benchmark default prompt",
                )
                .map_err(BenchmarkRunError::from_setup)?,
            )
            .await
            .map_err(BenchmarkRunError::from_setup)?;
        checkpoint_fixture_wal(&store, "Agent management fixture").await?;
        Ok(Self {
            agents,
            store,
            temporary_root,
        })
    }
}

async fn run_function_crud(
    target: &TargetSpec,
    environment: &EnvironmentFingerprint,
) -> Result<BenchmarkReport, BenchmarkRunError> {
    let fixture = FunctionBenchmarkFixture::open().await?;
    let store = fixture.functions.clone();
    let sequence = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    measure_local_async_checked(target.clone(), environment.clone(), move || {
        let store = store.clone();
        let sequence = std::sync::Arc::clone(&sequence);
        async move {
            let sequence = sequence.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let identifier = format!("benchmark_crud_{sequence:06}");
            let created = store
                .create(function_placeholder_input(
                    identifier.clone(),
                    format!("Benchmark CRUD {sequence:06}"),
                )?)
                .await?;
            let loaded = store.get(created.id()).await?.ok_or_else(|| {
                BenchmarkRunError::Operation("created Function was not materialized".into())
            })?;
            if loaded.identifier() != identifier {
                return Err(BenchmarkRunError::Operation(
                    "created Function identifier changed during materialization".into(),
                ));
            }
            let updated = store
                .update(
                    created.id(),
                    function_placeholder_input(
                        identifier,
                        format!("Benchmark CRUD updated {sequence:06}"),
                    )?,
                )
                .await?;
            store.delete(updated.id()).await?;
            if store.get(updated.id()).await?.is_some() {
                return Err(BenchmarkRunError::Operation(
                    "deleted Function remained materialized".into(),
                ));
            }
            Ok(())
        }
    })
    .await
}

async fn run_function_search_page_pair_v2(
    target: &TargetSpec,
    environment: &EnvironmentFingerprint,
) -> Result<BenchmarkReport, BenchmarkRunError> {
    let fixture = FunctionBenchmarkFixture::open().await?;
    fixture.set_fixture_synchronous(false).await?;
    for index in 0..FUNCTION_FIXTURE_ROWS {
        fixture
            .functions
            .create(function_placeholder_input(
                format!("function_fixture_{index:05}"),
                format!("Function fixture {index:05}"),
            )?)
            .await?;
    }
    fixture.set_fixture_synchronous(true).await?;

    let store = fixture.functions.clone();
    let sequence = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    measure_local_async_checked(target.clone(), environment.clone(), move || {
        let store = store.clone();
        let sequence = std::sync::Arc::clone(&sequence);
        async move {
            let index = sequence.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                % FUNCTION_SEARCH_PAGE_SCHEDULE.len();
            let step = FUNCTION_SEARCH_PAGE_SCHEDULE[index];
            for (search, page_number) in step.routes() {
                let page = store.list(search.map(str::to_owned), page_number).await?;
                let expected_total = if search.is_some() {
                    FUNCTION_FIXTURE_ROWS as i64
                } else {
                    FUNCTION_FIXTURE_ROWS as i64 + 4
                };
                if page.items().len() != 20
                    || page.page() != page_number
                    || page.page_size() != 20
                    || page.total() != expected_total
                {
                    return Err(BenchmarkRunError::Operation(format!(
                        "Function page contract drifted: search={search:?}, requested={page_number}, actual_page={}, page_size={}, rows={}, total={}, expected_total={expected_total}",
                        page.page(),
                        page.page_size(),
                        page.items().len(),
                        page.total(),
                    )));
                }
                let offset = usize::try_from((page_number - 1) * page.page_size())
                    .map_err(BenchmarkRunError::from_operation)?;
                for (item_index, item) in page.items().iter().enumerate() {
                    let expected_identifier =
                        expected_function_identifier(search.is_some(), offset + item_index)?;
                    if item.identifier() != expected_identifier {
                        return Err(BenchmarkRunError::Operation(format!(
                            "Function page order drifted: search={search:?}, requested={page_number}, row={item_index}, actual_identifier={}, expected_identifier={expected_identifier}",
                            item.identifier(),
                        )));
                    }
                }
            }
            Ok(())
        }
    })
    .await
}

fn expected_function_identifier(
    filtered: bool,
    ordered_position: usize,
) -> Result<String, BenchmarkRunError> {
    if filtered {
        return (ordered_position < FUNCTION_FIXTURE_ROWS)
            .then(|| format!("function_fixture_{ordered_position:05}"))
            .ok_or_else(|| {
                BenchmarkRunError::Operation(format!(
                    "filtered Function page position {ordered_position} exceeds fixture rows"
                ))
            });
    }

    // The immutable Builtin registry sorts by normalized display name around
    // the ASCII fixture names: Format Template, all Function fixtures, then
    // JSON Parse, JSON Stringify, and Regex Match. Keeping the exact identifier
    // at each measured row makes the benchmark reject duplicates, gaps, or an
    // unstable total order instead of approving any arbitrary 20-row page.
    match ordered_position {
        0 => Ok("format_template".to_string()),
        position if position <= FUNCTION_FIXTURE_ROWS => {
            Ok(format!("function_fixture_{:05}", position - 1))
        }
        position if position == FUNCTION_FIXTURE_ROWS + 1 => Ok("json_parse".to_string()),
        position if position == FUNCTION_FIXTURE_ROWS + 2 => Ok("json_stringify".to_string()),
        position if position == FUNCTION_FIXTURE_ROWS + 3 => Ok("text_regex_match".to_string()),
        position => Err(BenchmarkRunError::Operation(format!(
            "unfiltered Function page position {position} exceeds fixture rows and Builtins"
        ))),
    }
}

fn function_placeholder_input(
    identifier: String,
    name: String,
) -> Result<FunctionInput, BenchmarkRunError> {
    FunctionInput::for_write(
        identifier,
        name,
        None,
        FunctionKind::Placeholder,
        r#"{"type":"object"}"#.to_owned(),
        r#"{"type":"object"}"#.to_owned(),
        None,
        None,
        None,
        None,
    )
    .map_err(BenchmarkRunError::from_operation)
}

async fn run_conversation_list(
    target: &TargetSpec,
    environment: &EnvironmentFingerprint,
) -> Result<BenchmarkReport, BenchmarkRunError> {
    let fixture = ConversationBenchmarkFixture::open().await?;
    let mut seeded_sessions = Vec::with_capacity(CONVERSATION_FIXTURE_ROWS);
    for index in 0..CONVERSATION_FIXTURE_ROWS {
        let session = fixture
            .conversations
            .create_session_for_agent(
                fixture.agent_id,
                &format!("Conversation fixture {index:04}"),
            )
            .await
            .map_err(BenchmarkRunError::from_setup)?;
        seeded_sessions.push(session);
    }
    seeded_sessions.sort_by(|left, right| {
        let updated_at_order = right.updated_at().cmp(left.updated_at());
        updated_at_order.then_with(|| left.id().cmp(right.id()))
    });
    let expected_recent_ids = std::sync::Arc::new(
        seeded_sessions
            .iter()
            .take(20)
            .map(|session| session.id().to_owned())
            .collect::<Vec<_>>(),
    );
    let conversations = fixture.conversations.clone();
    let _fixture_lifetime = (fixture.store, fixture.temporary_root);
    measure_local_async_checked_batched(
        target.clone(),
        environment.clone(),
        CONVERSATION_LIST_OPERATIONS_PER_SAMPLE,
        move || {
            let conversations = conversations.clone();
            let expected_recent_ids = std::sync::Arc::clone(&expected_recent_ids);
            async move {
                let sessions = conversations
                    .list_recent_sessions()
                    .await
                    .map_err(BenchmarkRunError::from_operation)?;
                let actual_ids = sessions
                    .iter()
                    .map(|session| session.id().to_owned())
                    .collect::<Vec<_>>();
                let expected_recent_ids = expected_recent_ids.as_slice();
                if actual_ids != expected_recent_ids {
                    return Err(BenchmarkRunError::Operation(format!(
                        "Conversation recent page order drifted: actual_ids={actual_ids:?}, expected_recent_ids={expected_recent_ids:?}"
                    )));
                }
                Ok(())
            }
        },
    )
    .await
}

async fn run_conversation_bundle(
    target: &TargetSpec,
    environment: &EnvironmentFingerprint,
) -> Result<BenchmarkReport, BenchmarkRunError> {
    let fixture = ConversationBenchmarkFixture::open().await?;
    let mut ids = Vec::with_capacity(25);
    for index in 0..25 {
        let session = fixture
            .conversations
            .create_session_for_agent(fixture.agent_id, &format!("Bundle fixture {index:02}"))
            .await
            .map_err(BenchmarkRunError::from_setup)?;
        fixture
            .conversations
            .append_message(session.id(), "encrypted fixture message", None)
            .await
            .map_err(BenchmarkRunError::from_setup)?;
        fixture
            .conversations
            .record_execution(session.id(), ExecutionState::Completed)
            .await
            .map_err(BenchmarkRunError::from_setup)?;
        ids.push(session.id().to_string());
    }
    let ids = std::sync::Arc::new(ids);
    let conversations = fixture.conversations.clone();
    let _fixture_lifetime = (fixture.store, fixture.temporary_root);
    measure_local_async_checked_batched(
        target.clone(),
        environment.clone(),
        CONVERSATION_BUNDLE_OPERATIONS_PER_SAMPLE,
        move || {
            let ids = std::sync::Arc::clone(&ids);
            let conversations = conversations.clone();
            async move {
                let bundles = conversations
                    .load_session_bundles(ids.as_slice())
                    .await
                    .map_err(BenchmarkRunError::from_operation)?;
                if bundles.len() != ids.len()
                    || bundles.iter().any(|bundle| {
                        bundle.messages().len() != 1 || bundle.executions().len() != 1
                    })
                {
                    return Err(BenchmarkRunError::Operation(
                        "Conversation bundle cardinality drifted".into(),
                    ));
                }
                Ok(())
            }
        },
    )
    .await
}

async fn run_conversation_cleanup(
    target: &TargetSpec,
    environment: &EnvironmentFingerprint,
) -> Result<BenchmarkReport, BenchmarkRunError> {
    let fixture = ConversationBenchmarkFixture::open().await?;
    let conversations = fixture.conversations.clone();
    let agent_id = fixture.agent_id;
    let _fixture_lifetime = (fixture.store, fixture.temporary_root);
    measure_local_async_checked(target.clone(), environment.clone(), move || {
        let conversations = conversations.clone();
        async move {
            let session = conversations
                .create_session_for_agent(agent_id, "Retention benchmark")
                .await
                .map_err(BenchmarkRunError::from_operation)?;
            sqlx::query("UPDATE chat_sessions SET expires_at = ? WHERE id = ?")
                .bind("2000-01-01T00:00:00Z")
                .bind(session.id())
                .execute(conversations.pool())
                .await
                .map_err(BenchmarkRunError::from_operation)?;
            let preview = conversations
                .preview_retention("expired")
                .await
                .map_err(BenchmarkRunError::from_operation)?;
            if preview.affected_count() != 1 {
                return Err(BenchmarkRunError::Operation(format!(
                    "Conversation retention preview returned {} rows instead of 1",
                    preview.affected_count()
                )));
            }
            let deleted = conversations
                .apply_retention(&preview)
                .await
                .map_err(BenchmarkRunError::from_operation)?;
            if deleted != 1 {
                return Err(BenchmarkRunError::Operation(format!(
                    "Conversation retention deleted {deleted} rows instead of 1"
                )));
            }
            Ok(())
        }
    })
    .await
}

async fn run_conversation_recovery(
    target: &TargetSpec,
    environment: &EnvironmentFingerprint,
) -> Result<BenchmarkReport, BenchmarkRunError> {
    let fixture = ConversationBenchmarkFixture::open().await?;
    let conversations = fixture.conversations.clone();
    let agent_id = fixture.agent_id;
    let _fixture_lifetime = (fixture.store, fixture.temporary_root);
    measure_local_async_checked(target.clone(), environment.clone(), move || {
        let conversations = conversations.clone();
        async move {
            let session = conversations
                .create_session_for_agent(agent_id, "Recovery benchmark")
                .await
                .map_err(BenchmarkRunError::from_operation)?;
            conversations
                .record_execution_state(
                    session.id(),
                    agent_id,
                    ExecutionState::Running,
                    &serde_json::json!({"status":"running"}),
                )
                .await
                .map_err(BenchmarkRunError::from_operation)?;
            let recovered = conversations
                .recover_interrupted_running()
                .await
                .map_err(BenchmarkRunError::from_operation)?;
            let replayed = conversations
                .recover_interrupted_running()
                .await
                .map_err(BenchmarkRunError::from_operation)?;
            if recovered != 1 || replayed != 0 {
                return Err(BenchmarkRunError::Operation(format!(
                    "Conversation recovery drifted: recovered={recovered}, replayed={replayed}"
                )));
            }
            Ok(())
        }
    })
    .await
}

struct ConversationBenchmarkFixture {
    conversations: ConversationStore,
    agent_id: i64,
    store: Store,
    temporary_root: tempfile::TempDir,
}

impl ConversationBenchmarkFixture {
    async fn open() -> Result<Self, BenchmarkRunError> {
        let temporary_root = tempfile::tempdir().map_err(BenchmarkRunError::from_setup)?;
        let store = Store::open_local(StoreOpenOptions::new(
            temporary_root.path().join("hivegui.db"),
            temporary_root.path().join("plugins"),
        ))
        .await
        .map_err(BenchmarkRunError::from_setup)?;
        let agent_id = AgentStore::new(store.pool().clone())
            .map_err(BenchmarkRunError::from_setup)?
            .create(
                AgentInput::new_root(
                    "conversation_benchmark_root",
                    "Conversation benchmark root",
                    "local conversation benchmark prompt",
                )
                .map_err(BenchmarkRunError::from_setup)?,
            )
            .await
            .map_err(BenchmarkRunError::from_setup)?
            .id();
        checkpoint_fixture_wal(&store, "Conversation fixture").await?;
        let conversations =
            ConversationStore::from_store(&store, None).map_err(BenchmarkRunError::from_setup)?;
        Ok(Self {
            conversations,
            agent_id,
            store,
            temporary_root,
        })
    }
}

async fn checkpoint_fixture_wal(store: &Store, fixture: &str) -> Result<(), BenchmarkRunError> {
    let checkpoint: (i64, i64, i64) = sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(store.pool())
        .await
        .map_err(BenchmarkRunError::from_setup)?;
    if checkpoint != (0, 0, 0) {
        return Err(BenchmarkRunError::Setup(format!(
            "{fixture} WAL checkpoint did not converge: {checkpoint:?}"
        )));
    }
    Ok(())
}

async fn run_tool_crud(
    target: &TargetSpec,
    environment: &EnvironmentFingerprint,
) -> Result<BenchmarkReport, BenchmarkRunError> {
    let fixture = ToolBenchmarkFixture::open().await?;
    let store = fixture.tools.clone();
    let target_fixture = fixture.target.clone();
    let sequence = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    measure_local_async_checked(target.clone(), environment.clone(), move || {
        let store = store.clone();
        let target_fixture = target_fixture.clone();
        let sequence = std::sync::Arc::clone(&sequence);
        async move {
            let sequence = sequence.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let identifier = format!("benchmark_tool_crud_{sequence:06}");
            let created = store
                .create(tool_function_input(
                    identifier.clone(),
                    format!("Benchmark Tool CRUD {sequence:06}"),
                    &target_fixture,
                )?)
                .await?;
            let loaded = store.get(created.id()).await?.ok_or_else(|| {
                BenchmarkRunError::Operation("created Tool was not materialized".into())
            })?;
            if loaded.identifier() != identifier {
                return Err(BenchmarkRunError::Operation(
                    "created Tool identifier changed during materialization".into(),
                ));
            }
            let updated = store
                .update(
                    created.id(),
                    tool_function_input(
                        identifier,
                        format!("Benchmark Tool CRUD updated {sequence:06}"),
                        &target_fixture,
                    )?,
                )
                .await?;
            store.delete(updated.id()).await?;
            if store.get(updated.id()).await?.is_some() {
                return Err(BenchmarkRunError::Operation(
                    "deleted Tool remained materialized".into(),
                ));
            }
            Ok(())
        }
    })
    .await
}

async fn run_tool_search_page_pair_v2(
    target: &TargetSpec,
    environment: &EnvironmentFingerprint,
) -> Result<BenchmarkReport, BenchmarkRunError> {
    let fixture = ToolBenchmarkFixture::open().await?;
    fixture.set_fixture_synchronous(false).await?;
    for index in 0..TOOL_FIXTURE_ROWS {
        fixture
            .tools
            .create(tool_function_input(
                format!("tool_fixture_{index:05}"),
                format!("Tool fixture {index:05}"),
                &fixture.target,
            )?)
            .await?;
    }
    fixture.set_fixture_synchronous(true).await?;

    let store = fixture.tools.clone();
    let sequence = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    measure_local_async_checked(target.clone(), environment.clone(), move || {
        let store = store.clone();
        let sequence = std::sync::Arc::clone(&sequence);
        async move {
            let index = sequence.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                % TOOL_SEARCH_PAGE_SCHEDULE.len();
            let step = TOOL_SEARCH_PAGE_SCHEDULE[index];
            for (search, requested_page) in step.routes() {
                let page = store.list(search.map(str::to_owned), requested_page).await?;
                if page.items().len() != 20
                    || page.page() != requested_page
                    || page.page_size() != 20
                    || page.total() != TOOL_FIXTURE_ROWS as i64
                {
                    return Err(BenchmarkRunError::Operation(format!(
                        "Tool page contract drifted: search={search:?}, requested={requested_page}, actual_page={}, page_size={}, rows={}, total={}",
                        page.page(),
                        page.page_size(),
                        page.items().len(),
                        page.total(),
                    )));
                }
                let offset = usize::try_from((requested_page - 1) * page.page_size())
                    .map_err(BenchmarkRunError::from_operation)?;
                for (item_index, item) in page.items().iter().enumerate() {
                    let expected = format!("tool_fixture_{:05}", offset + item_index);
                    if item.identifier() != expected {
                        return Err(BenchmarkRunError::Operation(format!(
                            "Tool page order drifted: search={search:?}, requested={requested_page}, row={item_index}, actual_identifier={}, expected_identifier={expected}",
                            item.identifier(),
                        )));
                    }
                }
            }
            Ok(())
        }
    })
    .await
}

#[derive(Debug, Clone)]
struct ToolBenchmarkTarget {
    function_id: i64,
    input_schema: String,
    output_schema: String,
}

fn tool_function_input(
    identifier: String,
    name: String,
    target: &ToolBenchmarkTarget,
) -> Result<ToolInput, BenchmarkRunError> {
    ToolInput::for_write(
        identifier,
        name,
        "Tool management benchmark fixture".to_string(),
        ToolKind::FunctionWrap,
        ToolSource::Workspace,
        false,
        Some(target.function_id),
        None,
        target.input_schema.clone(),
        target.output_schema.clone(),
        None,
        None,
    )
    .map_err(BenchmarkRunError::from_operation)
}

struct FunctionBenchmarkFixture {
    functions: FunctionStore,
    _store: Store,
    _temporary_root: tempfile::TempDir,
}

struct ToolBenchmarkFixture {
    tools: ToolStore,
    target: ToolBenchmarkTarget,
    store: Store,
    _temporary_root: tempfile::TempDir,
}

impl ToolBenchmarkFixture {
    async fn open() -> Result<Self, BenchmarkRunError> {
        let temporary_root = tempfile::tempdir().map_err(BenchmarkRunError::from_setup)?;
        let database_path = temporary_root.path().join("hivegui.db");
        let plugin_root = temporary_root.path().join("plugins");
        let store = Store::open_local(StoreOpenOptions::new(database_path, plugin_root))
            .await
            .map_err(BenchmarkRunError::from_setup)?;
        let tools = ToolStore::new(store.pool().clone()).map_err(BenchmarkRunError::from_setup)?;
        let (function_id, input_schema, output_schema) =
            sqlx::query_as::<_, (i64, String, String)>(
                "SELECT id, input_schema, output_schema FROM functions \
                 WHERE identifier = 'format_template'",
            )
            .fetch_one(store.pool())
            .await
            .map_err(BenchmarkRunError::from_setup)?;
        Ok(Self {
            tools,
            target: ToolBenchmarkTarget {
                function_id,
                input_schema,
                output_schema,
            },
            store,
            _temporary_root: temporary_root,
        })
    }

    async fn set_fixture_synchronous(&self, durable: bool) -> Result<(), BenchmarkRunError> {
        let pool = self.store.pool();
        let connection_count = usize::try_from(pool.options().get_max_connections())
            .map_err(BenchmarkRunError::from_setup)?;
        let mut connections = Vec::with_capacity(connection_count);
        for _ in 0..connection_count {
            connections.push(
                pool.acquire()
                    .await
                    .map_err(BenchmarkRunError::from_setup)?,
            );
        }
        let pragma = if durable {
            "PRAGMA synchronous = FULL"
        } else {
            "PRAGMA synchronous = OFF"
        };
        for connection in &mut connections {
            sqlx::query(pragma)
                .execute(&mut **connection)
                .await
                .map_err(BenchmarkRunError::from_setup)?;
        }
        drop(connections);
        if durable {
            sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                .execute(pool)
                .await
                .map_err(BenchmarkRunError::from_setup)?;
        }
        Ok(())
    }
}

impl FunctionBenchmarkFixture {
    async fn open() -> Result<Self, BenchmarkRunError> {
        let temporary_root = tempfile::tempdir().map_err(BenchmarkRunError::from_setup)?;
        let database_path = temporary_root.path().join("hivegui.db");
        let plugin_root = temporary_root.path().join("plugins");
        let store = Store::open_local(StoreOpenOptions::new(database_path, plugin_root))
            .await
            .map_err(BenchmarkRunError::from_setup)?;
        let functions =
            FunctionStore::new(store.pool().clone()).map_err(BenchmarkRunError::from_setup)?;
        Ok(Self {
            functions,
            _store: store,
            _temporary_root: temporary_root,
        })
    }

    async fn set_fixture_synchronous(&self, durable: bool) -> Result<(), BenchmarkRunError> {
        // Fixture population is explicitly outside the measured boundary. Set
        // every connection in the isolated Store pool to synchronous=OFF for
        // the 10,000 typed writes, then restore production durability before
        // the first warmup query. The data and production FunctionStore path
        // are unchanged; this only avoids spending minutes fsyncing disposable
        // setup transactions.
        let pool = self._store.pool();
        let connection_count = usize::try_from(pool.options().get_max_connections())
            .map_err(BenchmarkRunError::from_setup)?;
        let mut connections = Vec::with_capacity(connection_count);
        for _ in 0..connection_count {
            connections.push(
                pool.acquire()
                    .await
                    .map_err(BenchmarkRunError::from_setup)?,
            );
        }
        let pragma = if durable {
            "PRAGMA synchronous = FULL"
        } else {
            "PRAGMA synchronous = OFF"
        };
        for connection in &mut connections {
            sqlx::query(pragma)
                .execute(&mut **connection)
                .await
                .map_err(BenchmarkRunError::from_setup)?;
        }
        drop(connections);
        if durable {
            sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                .execute(pool)
                .await
                .map_err(BenchmarkRunError::from_setup)?;
        }
        Ok(())
    }
}

async fn measure_local_async_checked<F, Fut, R>(
    target: TargetSpec,
    environment: EnvironmentFingerprint,
    mut operation: F,
) -> Result<BenchmarkReport, BenchmarkRunError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<R, BenchmarkRunError>>,
{
    for _ in 0..target.warmup_iterations {
        black_box(operation().await?);
    }

    let mut samples = Vec::with_capacity(target.measured_samples);
    for _ in 0..target.measured_samples {
        let started = Instant::now();
        black_box(operation().await?);
        samples.push(duration_ns(started)?);
    }
    BenchmarkReport::from_samples(target, environment, &samples).map_err(Into::into)
}

/// Measure a checked fast asynchronous boundary in fixed batches, normalize
/// each sample to one operation, and fail immediately on any operation error.
pub async fn measure_local_async_checked_batched<F, Fut, R>(
    target: TargetSpec,
    environment: EnvironmentFingerprint,
    operations_per_sample: usize,
    mut operation: F,
) -> Result<BenchmarkReport, BenchmarkRunError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<R, BenchmarkRunError>>,
{
    let divisor = u64::try_from(operations_per_sample)
        .ok()
        .filter(|divisor| *divisor > 0)
        .ok_or(MeasurementError::InvalidBatchSize)?;

    for _ in 0..target.warmup_iterations {
        for _ in 0..operations_per_sample {
            black_box(operation().await?);
        }
    }

    let mut samples = Vec::with_capacity(target.measured_samples);
    for _ in 0..target.measured_samples {
        let started = Instant::now();
        for _ in 0..operations_per_sample {
            black_box(operation().await?);
        }
        samples.push(duration_ns(started)? / divisor);
    }
    BenchmarkReport::from_samples(target, environment, &samples).map_err(Into::into)
}

#[derive(Debug)]
pub enum BenchmarkRunError {
    Setup(String),
    Operation(String),
    Measurement(MeasurementError),
}

impl BenchmarkRunError {
    fn from_setup(error: impl fmt::Display) -> Self {
        Self::Setup(error.to_string())
    }

    fn from_operation(error: impl fmt::Display) -> Self {
        Self::Operation(error.to_string())
    }
}

impl fmt::Display for BenchmarkRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Setup(error) => write!(formatter, "benchmark setup failed: {error}"),
            Self::Operation(error) => write!(formatter, "benchmark operation failed: {error}"),
            Self::Measurement(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for BenchmarkRunError {}

impl From<MeasurementError> for BenchmarkRunError {
    fn from(error: MeasurementError) -> Self {
        Self::Measurement(error)
    }
}

impl From<hivegui::datasource::validation::PublicBoundaryError> for BenchmarkRunError {
    fn from(error: hivegui::datasource::validation::PublicBoundaryError) -> Self {
        Self::Operation(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeasurementError {
    NoSamples,
    WrongSampleCount { expected: usize, actual: usize },
    InvalidBatchSize,
    DurationOverflow,
}

impl fmt::Display for MeasurementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSamples => write!(formatter, "benchmark produced no samples"),
            Self::WrongSampleCount { expected, actual } => write!(
                formatter,
                "benchmark requires {expected} measured samples, received {actual}"
            ),
            Self::InvalidBatchSize => {
                write!(formatter, "benchmark batch size must be greater than zero")
            }
            Self::DurationOverflow => write!(formatter, "benchmark duration exceeded u64 nanos"),
        }
    }
}

impl std::error::Error for MeasurementError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompatibilityError {
    SchemaVersion,
    FixtureVersion,
    Target,
    Environment,
    SampleCount,
    BaselineApproval(&'static str),
}

impl fmt::Display for CompatibilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SchemaVersion => write!(formatter, "baseline schema version mismatch"),
            Self::FixtureVersion => write!(formatter, "benchmark fixture version mismatch"),
            Self::Target => write!(formatter, "benchmark target definition mismatch"),
            Self::Environment => write!(formatter, "benchmark environment fingerprint mismatch"),
            Self::SampleCount => write!(formatter, "benchmark sample count mismatch"),
            Self::BaselineApproval(field) => {
                write!(
                    formatter,
                    "baseline approval field {field} is missing or invalid"
                )
            }
        }
    }
}

impl std::error::Error for CompatibilityError {}

fn validate_compatibility(
    current: &BenchmarkReport,
    baseline: &BenchmarkBaseline,
) -> Result<(), CompatibilityError> {
    let stored = &baseline.report;
    if current.schema_version != BASELINE_SCHEMA_VERSION
        || stored.schema_version != BASELINE_SCHEMA_VERSION
    {
        return Err(CompatibilityError::SchemaVersion);
    }
    if current.fixture_version != FIXTURE_VERSION || stored.fixture_version != FIXTURE_VERSION {
        return Err(CompatibilityError::FixtureVersion);
    }
    if current.target != stored.target {
        return Err(CompatibilityError::Target);
    }
    if current.environment != stored.environment {
        return Err(CompatibilityError::Environment);
    }
    if current.sample_count != stored.sample_count
        || current.sample_count != current.target.measured_samples
    {
        return Err(CompatibilityError::SampleCount);
    }
    for (field, value) in [
        ("reviewer", baseline.approval.reviewer.as_str()),
        ("approved_at", baseline.approval.approved_at.as_str()),
        (
            "source_revision",
            baseline.approval.source_revision.as_str(),
        ),
    ] {
        if value.trim().is_empty() {
            return Err(CompatibilityError::BaselineApproval(field));
        }
    }
    if NaiveDate::parse_from_str(&baseline.approval.approved_at, "%Y-%m-%d").is_err() {
        return Err(CompatibilityError::BaselineApproval("approved_at"));
    }
    if !valid_source_revision(&baseline.approval.source_revision) {
        return Err(CompatibilityError::BaselineApproval("source_revision"));
    }
    Ok(())
}

fn valid_source_revision(value: &str) -> bool {
    let Some((head, source)) = value
        .strip_prefix("git:")
        .and_then(|value| value.split_once("+hivegui-source-v1:"))
    else {
        return false;
    };
    head.len() == 40
        && source.len() == 64
        && head.bytes().all(|byte| byte.is_ascii_hexdigit())
        && source.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn regressions(baseline: PercentilesNs, current: PercentilesNs) -> Vec<Regression> {
    [
        (Percentile::P50, baseline.p50, current.p50),
        (Percentile::P95, baseline.p95, current.p95),
        (Percentile::P99, baseline.p99, current.p99),
    ]
    .into_iter()
    .filter(|(_, baseline, current)| exceeds_regression_limit(*baseline, *current))
    .map(|(percentile, baseline_ns, current_ns)| Regression {
        percentile,
        baseline_ns,
        current_ns,
    })
    .collect()
}

fn exceeds_regression_limit(baseline: u64, current: u64) -> bool {
    if baseline == 0 {
        return current > 0;
    }
    u128::from(current) * 100 > u128::from(baseline) * u128::from(100 + REGRESSION_LIMIT_PERCENT)
}

fn validate_bound_exception(
    current: &BenchmarkReport,
    current_source_revision: &str,
    baseline: &BenchmarkBaseline,
    actual_regressions: &[Regression],
    exception: &RegressionException,
    as_of: NaiveDate,
) -> Result<(), String> {
    if exception.schema_version != REGRESSION_EXCEPTION_SCHEMA_VERSION {
        return Err("regression exception schema version mismatch".into());
    }
    if exception.target != current.target {
        return Err("regression exception target mismatch".into());
    }
    if exception.environment != current.environment {
        return Err("regression exception environment mismatch".into());
    }
    if !valid_source_revision(current_source_revision) {
        return Err("current source revision is malformed".into());
    }
    if !valid_source_revision(&exception.current_source_revision) {
        return Err("regression exception current source revision is malformed".into());
    }
    if exception.current_source_revision != current_source_revision {
        return Err("regression exception current source revision mismatch".into());
    }
    if exception.baseline.approval != baseline.approval {
        return Err("regression exception baseline approval mismatch".into());
    }
    if exception.baseline.percentiles_ns != baseline.report.percentiles_ns {
        return Err("regression exception baseline percentiles mismatch".into());
    }

    let approval = &exception.approval;
    for (field, value) in [
        ("signer", approval.signer.as_str()),
        ("approved_at", approval.approved_at.as_str()),
        ("reason", approval.reason.as_str()),
        ("impact_scope", approval.impact_scope.as_str()),
        ("review_due", approval.review_due.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(format!("regression exception field {field} is missing"));
        }
    }

    let approved_at = NaiveDate::parse_from_str(&approval.approved_at, "%Y-%m-%d")
        .map_err(|_| "regression exception approved_at must be YYYY-MM-DD".to_owned())?;
    let review_due = NaiveDate::parse_from_str(&approval.review_due, "%Y-%m-%d")
        .map_err(|_| "regression exception review_due must be YYYY-MM-DD".to_owned())?;
    if approved_at.format("%Y-%m-%d").to_string() != approval.approved_at {
        return Err("regression exception approved_at must be canonical YYYY-MM-DD".into());
    }
    if review_due.format("%Y-%m-%d").to_string() != approval.review_due {
        return Err("regression exception review_due must be canonical YYYY-MM-DD".into());
    }
    if review_due < approved_at {
        return Err("regression exception review_due precedes approved_at".into());
    }
    if approved_at > as_of {
        return Err("regression exception approved_at is in the future".into());
    }
    if review_due < as_of {
        return Err("regression exception review_due has expired".into());
    }

    let mut approved_percentiles = Vec::with_capacity(exception.approved_regressions.len());
    for approved in &exception.approved_regressions {
        if approved_percentiles.contains(&approved.percentile) {
            return Err(format!(
                "regression exception contains duplicate {:?} approval",
                approved.percentile
            ));
        }
        approved_percentiles.push(approved.percentile);

        let expected_baseline =
            percentile_value(baseline.report.percentiles_ns, approved.percentile);
        if approved.baseline_ns == 0 {
            return Err(format!(
                "regression exception {:?} baseline must be greater than zero",
                approved.percentile
            ));
        }
        if approved.baseline_ns != expected_baseline {
            return Err(format!(
                "regression exception {:?} baseline mismatch",
                approved.percentile
            ));
        }
        if !exceeds_regression_limit(approved.baseline_ns, approved.observed_current_ns) {
            return Err(format!(
                "regression exception {:?} observation is not strictly above the 10% limit",
                approved.percentile
            ));
        }
        if approved.observed_current_ns > approved.approved_max_current_ns {
            return Err(format!(
                "regression exception {:?} approved maximum is below its observed regression",
                approved.percentile
            ));
        }
        if approved.percentile == Percentile::P95
            && approved.approved_max_current_ns > current.target.p95_budget_ns
        {
            return Err("regression exception p95 maximum exceeds the absolute p95 budget".into());
        }
    }

    for actual in actual_regressions {
        let Some(approved) = exception
            .approved_regressions
            .iter()
            .find(|approved| approved.percentile == actual.percentile)
        else {
            return Err(format!(
                "regression exception does not approve {:?}",
                actual.percentile
            ));
        };
        if approved.baseline_ns != actual.baseline_ns {
            return Err(format!(
                "regression exception {:?} baseline mismatch",
                actual.percentile
            ));
        }
        if actual.current_ns > approved.approved_max_current_ns {
            return Err(format!(
                "regression exception {:?} current value exceeds approved maximum",
                actual.percentile
            ));
        }
    }
    Ok(())
}

fn percentile_value(percentiles: PercentilesNs, percentile: Percentile) -> u64 {
    match percentile {
        Percentile::P50 => percentiles.p50,
        Percentile::P95 => percentiles.p95,
        Percentile::P99 => percentiles.p99,
    }
}

fn nearest_rank(sorted: &[u64], percentile: usize) -> u64 {
    let rank = (percentile * sorted.len()).div_ceil(100).max(1);
    sorted[rank - 1]
}

fn duration_ns(started: Instant) -> Result<u64, MeasurementError> {
    u64::try_from(started.elapsed().as_nanos()).map_err(|_| MeasurementError::DurationOverflow)
}

fn command_output(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program).args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(target_os = "linux")]
fn cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                matches!(name.trim(), "model name" | "Hardware").then(|| value.trim().to_owned())
            })
        })
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(target_os = "macos")]
fn cpu_model() -> String {
    command_output("sysctl", &["-n", "machdep.cpu.brand_string"])
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(target_os = "windows")]
fn cpu_model() -> String {
    std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unknown".into())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn cpu_model() -> String {
    "unknown".into()
}

fn slug(value: &str) -> String {
    let slug = value
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    slug.split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}
