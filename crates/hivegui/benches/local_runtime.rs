#[path = "../tests/support/performance.rs"]
mod performance;

use performance::{EnvironmentFingerprint, TargetSpec, baseline_path, target_specs};

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
