//! T015 Red contract for the Foundation local background-execution boundary.
//!
//! Full local conversations and zero-request/fallback capture remain owned by
//! T116/T140. This file deliberately limits itself to dependency direction,
//! construction, and one injected adapter dispatch through the planned T026
//! registry.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde_json::Value;

use hivegui::runtime::FoundationRuntimeComposition;
use hivegui::runtime::execution::{
    LocalExecutionAdapter, LocalExecutionFuture, LocalExecutionOutcome, LocalExecutionRequest,
};

const CORE_MANIFEST: &str = include_str!("../../hive-runtime-core/Cargo.toml");
const HIVEGUI_MANIFEST: &str = include_str!("../Cargo.toml");
const FOUNDATION_RUNTIME_MOD_SOURCE: &str = include_str!("../src/runtime/mod.rs");
const FOUNDATION_APP_SOURCE: &str = include_str!("../src/ui/app.rs");
const HIVEGUI_MAIN_SOURCE: &str = include_str!("../src/main.rs");
const HIVEGUI_CONFIG_SOURCE: &str = include_str!("../src/config.rs");

const ALLOWED_CORE_DIRECT_DEPENDENCIES: &[&str] = &[
    "anyhow",
    "async-trait",
    "chrono",
    "extism",
    "futures",
    "serde",
    "serde-json",
    "sha2",
    "thiserror",
    "tokio",
    "tokio-util",
    "tracing",
    "uuid",
    "wasmparser",
    "wasmtime",
];

fn dependency_names(manifest: &str) -> BTreeSet<String> {
    let mut in_dependency_section = false;
    let mut names = BTreeSet::new();

    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') {
            in_dependency_section = line == "[dependencies]"
                || line.ends_with(".dependencies]")
                || line.starts_with("[target.") && line.contains(".dependencies]");
            continue;
        }
        if !in_dependency_section || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, _)) = line.split_once('=') {
            names.insert(name.trim().replace('_', "-"));
        }
    }

    names
}

fn locked_offline_metadata() -> Value {
    let workspace_manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("Cargo.toml");
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--locked",
            "--offline",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(&workspace_manifest)
        .output()
        .expect("run cargo metadata for the committed offline dependency graph");
    assert!(
        output.status.success(),
        "cargo metadata --locked --offline must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("cargo metadata emitted valid JSON")
}

fn reachable_package_names(metadata: &Value, root_name: &str) -> BTreeSet<String> {
    let packages = metadata["packages"]
        .as_array()
        .expect("metadata packages is an array");
    let names_by_id = packages
        .iter()
        .map(|package| {
            (
                package["id"].as_str().expect("package id").to_owned(),
                package["name"].as_str().expect("package name").to_owned(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let root_ids = names_by_id
        .iter()
        .filter_map(|(id, name)| (name == root_name).then_some(id.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        root_ids.len(),
        1,
        "metadata must contain exactly one {root_name} package"
    );

    let adjacency = metadata["resolve"]["nodes"]
        .as_array()
        .expect("metadata resolve nodes is an array")
        .iter()
        .map(|node| {
            let dependencies = node["dependencies"]
                .as_array()
                .expect("resolve node dependencies is an array")
                .iter()
                .map(|dependency| {
                    dependency
                        .as_str()
                        .expect("dependency package id")
                        .to_owned()
                })
                .collect::<Vec<_>>();
            (
                node["id"].as_str().expect("resolve node id").to_owned(),
                dependencies,
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut pending = VecDeque::from(root_ids);
    let mut visited = BTreeSet::new();
    while let Some(package_id) = pending.pop_front() {
        if !visited.insert(package_id.clone()) {
            continue;
        }
        pending.extend(
            adjacency
                .get(&package_id)
                .expect("every package in the graph has a resolve node")
                .iter()
                .cloned(),
        );
    }

    visited
        .into_iter()
        .map(|id| {
            names_by_id
                .get(&id)
                .unwrap_or_else(|| panic!("resolve package {id} must have metadata"))
                .clone()
        })
        .collect()
}

fn workspace_package_names(metadata: &Value) -> BTreeSet<String> {
    let names_by_id = metadata["packages"]
        .as_array()
        .expect("metadata packages is an array")
        .iter()
        .map(|package| {
            (
                package["id"].as_str().expect("package id"),
                package["name"].as_str().expect("package name"),
            )
        })
        .collect::<BTreeMap<_, _>>();

    metadata["workspace_members"]
        .as_array()
        .expect("metadata workspace_members is an array")
        .iter()
        .map(|member| {
            let id = member.as_str().expect("workspace member package id");
            names_by_id
                .get(id)
                .unwrap_or_else(|| panic!("workspace member {id} has package metadata"))
                .to_string()
        })
        .collect()
}

fn is_forbidden_core_dependency(package_name: &str) -> bool {
    let normalized = package_name.replace('_', "-");
    normalized == "sqlx"
        || normalized.starts_with("sqlx-")
        || matches!(
            normalized.as_str(),
            "axum"
                | "axum-core"
                | "h2"
                | "http"
                | "http-body"
                | "http-body-util"
                | "hyper"
                | "hyper-rustls"
                | "hyper-tls"
                | "hyper-util"
                | "mysql-async"
                | "reqwest"
                | "rusqlite"
                | "tower-http"
                | "ureq"
                | "hiveclaw"
                | "hivegui"
                | "hiveweb"
        )
}

#[test]
fn shared_runtime_core_direct_dependencies_are_an_exact_pure_runtime_subset() {
    let dependencies = dependency_names(CORE_MANIFEST);
    let allowed = ALLOWED_CORE_DIRECT_DEPENDENCIES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    let outside_allowlist = dependencies.difference(&allowed).collect::<Vec<_>>();
    assert!(
        outside_allowlist.is_empty(),
        "hive-runtime-core direct dependencies fall outside the exact pure-runtime allowlist: {outside_allowlist:?}"
    );
}

#[test]
fn hivegui_manifest_has_no_hiveweb_dependency() {
    assert!(
        !dependency_names(HIVEGUI_MANIFEST).contains("hiveweb"),
        "HiveGUI and HiveWeb are independent deployables"
    );
}

#[test]
fn resolved_dependency_graph_preserves_core_neutrality_and_hivegui_independence() {
    let metadata = locked_offline_metadata();
    let core_reachable = reachable_package_names(&metadata, "hive-runtime-core");
    let workspace_packages = workspace_package_names(&metadata);
    let reached_product_crates = core_reachable
        .intersection(&workspace_packages)
        .filter(|name| name.as_str() != "hive-runtime-core")
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        reached_product_crates.is_empty(),
        "hive-runtime-core must not depend on any product/workspace crate: {reached_product_crates:?}"
    );
    let forbidden_core = core_reachable
        .iter()
        .filter(|name| name.as_str() != "hive-runtime-core")
        .filter(|name| is_forbidden_core_dependency(name))
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        forbidden_core.is_empty(),
        "hive-runtime-core resolved graph reached forbidden crates: {forbidden_core:?}"
    );

    let hivegui_reachable = reachable_package_names(&metadata, "hivegui");
    assert!(
        !hivegui_reachable.contains("hiveweb"),
        "HiveGUI resolved graph must never reach the HiveWeb product crate"
    );
}

fn code_and_literals(source: &str) -> String {
    source
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("//")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn visit_rust_sources(directory: &Path, sources: &mut Vec<(PathBuf, String)>) {
    for entry in fs::read_dir(directory).expect("read Foundation source directory") {
        let entry = entry.expect("read Foundation source entry");
        let path = entry.path();
        if path.is_dir() {
            visit_rust_sources(&path, sources);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = fs::read_to_string(&path).expect("read Foundation Rust source");
            sources.push((path, source));
        }
    }
}

fn foundation_composition_sources() -> Vec<(PathBuf, String)> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    visit_rust_sources(&manifest_dir.join("src/runtime"), &mut sources);
    for relative in ["src/ui/app.rs", "src/main.rs", "src/config.rs"] {
        let path = manifest_dir.join(relative);
        sources.push((
            path.clone(),
            fs::read_to_string(&path).expect("read HiveGUI composition entry"),
        ));
    }
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
}

#[test]
fn foundation_composition_exposes_local_injection_and_has_no_hiveweb_construction() {
    assert!(
        FOUNDATION_RUNTIME_MOD_SOURCE.contains("pub mod execution;"),
        "the Foundation runtime composition must expose its local execution boundary"
    );
    assert!(
        FOUNDATION_RUNTIME_MOD_SOURCE.contains("FoundationRuntimeComposition"),
        "runtime/mod.rs must expose the real HiveGUI Foundation composition factory"
    );
    assert!(
        FOUNDATION_APP_SOURCE.contains("FoundationRuntimeComposition"),
        "ui/app.rs must construct the Foundation runtime through the composition factory"
    );

    for (path, source) in foundation_composition_sources() {
        let normalized = code_and_literals(&source)
            .to_ascii_lowercase()
            .replace('_', "");
        assert!(
            !normalized.contains("hiveweb"),
            "Foundation composition at {} references HiveWeb code, client, URL/env configuration, or fallback",
            path.display()
        );
    }

    // Keep the entrypoint/config files compile-time-bound to this contract as
    // well as traversing them above; this catches accidental path moves.
    assert!(!HIVEGUI_MAIN_SOURCE.is_empty());
    assert!(!HIVEGUI_CONFIG_SOURCE.is_empty());
}

#[derive(Default)]
struct RecordingLocalAdapter {
    calls: AtomicUsize,
    requests: Mutex<Vec<LocalExecutionRequest>>,
}

impl LocalExecutionAdapter for RecordingLocalAdapter {
    fn execute(&self, request: LocalExecutionRequest) -> LocalExecutionFuture {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests
            .lock()
            .expect("recording adapter mutex poisoned")
            .push(request);
        Box::pin(async { Ok(LocalExecutionOutcome::Completed) })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hivegui_composition_factory_dispatches_only_to_the_injected_local_adapter() {
    let adapter = Arc::new(RecordingLocalAdapter::default());
    let composition = FoundationRuntimeComposition::with_local_adapter(adapter.clone(), 16)
        .expect("construct the real HiveGUI composition with an injected local adapter");
    let request = LocalExecutionRequest::new(
        "24ef4422-fcd0-4e57-8a30-1f51010b7ef7",
        "ec919552-35ec-49d6-a3f7-f2aa86916904",
        "local foundation probe",
    )
    .expect("valid local execution request");

    let execution_id = composition
        .dispatch_background(request.clone())
        .await
        .expect("dispatch local background execution through HiveGUI composition");
    let outcome = composition
        .wait_for_terminal(&execution_id)
        .await
        .expect("observe terminal local execution");

    assert_eq!(outcome, LocalExecutionOutcome::Completed);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        adapter
            .requests
            .lock()
            .expect("recording adapter mutex poisoned")
            .as_slice(),
        [request]
    );
}
