//! Shared benchmark schema, measurement helpers, and regression comparison.
//!
//! This module intentionally does not create or update baseline files. A new
//! target becomes eligible for a baseline only after its owning task has an
//! approved Green result; baseline creation is then an explicit review action.

use std::{
    fmt,
    future::Future,
    hint::black_box,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

pub const BASELINE_SCHEMA_VERSION: u32 = 1;
pub const FIXTURE_VERSION: &str = "hivegui-local-runtime-v1";
pub const BASELINE_ROOT: &str = "crates/hivegui/benches/baselines/v1";
pub const MEASURED_SAMPLES: usize = 100;
pub const WARMUP_ITERATIONS: usize = 10;
pub const REGRESSION_LIMIT_PERCENT: u64 = 10;

pub const AGENT_ACTION_DISPATCH_ID: &str = "agent_action_dispatch";
pub const TOOL_DISPATCH_ID: &str = "tool_dispatch";
pub const WORKFLOW_100_NODE_ID: &str = "workflow_100_node_noop";

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

pub fn target_specs() -> [TargetSpec; 3] {
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
            id: TOOL_DISPATCH_ID.into(),
            owner_task: "T103".into(),
            timing_boundary: "Tool validation complete to local executor started".into(),
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RegressionException {
    pub signer: String,
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
    as_of: NaiveDate,
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
        Some(exception) => match validate_exception(exception, as_of) {
            Ok(()) => Ok(ComparisonResult {
                outcome: ComparisonOutcome::ApprovedException,
                regressions,
                exception_rejection: None,
            }),
            Err(reason) => Ok(ComparisonResult {
                outcome: ComparisonOutcome::Blocked,
                regressions,
                exception_rejection: Some(reason),
            }),
        },
        None => Ok(ComparisonResult {
            outcome: ComparisonOutcome::Blocked,
            regressions,
            exception_rejection: None,
        }),
    }
}

pub fn baseline_path(target: &TargetSpec, environment: &EnvironmentFingerprint) -> PathBuf {
    Path::new(BASELINE_ROOT)
        .join(&target.id)
        .join(format!("{}.json", environment.baseline_key()))
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeasurementError {
    NoSamples,
    WrongSampleCount { expected: usize, actual: usize },
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
                write!(formatter, "baseline approval field {field} is missing")
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
    Ok(())
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

fn validate_exception(exception: &RegressionException, as_of: NaiveDate) -> Result<(), String> {
    for (field, value) in [
        ("signer", exception.signer.as_str()),
        ("reason", exception.reason.as_str()),
        ("impact_scope", exception.impact_scope.as_str()),
        ("review_due", exception.review_due.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(format!("regression exception field {field} is missing"));
        }
    }
    let review_due = NaiveDate::parse_from_str(&exception.review_due, "%Y-%m-%d")
        .map_err(|_| "regression exception review_due must be YYYY-MM-DD".to_owned())?;
    if review_due < as_of {
        return Err("regression exception review_due has expired".into());
    }
    Ok(())
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
