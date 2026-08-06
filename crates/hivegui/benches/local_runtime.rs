#[path = "../tests/support/performance.rs"]
mod performance;

use performance::{EnvironmentFingerprint, baseline_path, target_specs};

fn main() {
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
