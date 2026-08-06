mod common;

use std::path::Path;

fn workspace_file(relative_path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative_path);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn workflow_job<'a>(workflow: &'a str, job_name: &str, next_job: Option<&str>) -> &'a str {
    let marker = format!("  {job_name}:\n");
    let job_and_tail = workflow
        .split_once(&marker)
        .unwrap_or_else(|| panic!("CI workflow must define the `{job_name}` job"))
        .1;

    next_job
        .and_then(|next_job| {
            job_and_tail
                .split_once(&format!("\n  {next_job}:\n"))
                .map(|(job, _)| job)
        })
        .unwrap_or(job_and_tail)
}

fn workflow_step_containing<'a>(job: &'a str, needle: &str) -> &'a str {
    job.split("\n      - ")
        .find(|step| step.contains(needle))
        .unwrap_or_else(|| panic!("CI job must contain a step with `{needle}`"))
}

#[test]
fn accepts_only_loopback_disposable_test_databases() {
    for url in [
        "mysql://root@127.0.0.1:3306/hiveweb_test",
        "mysql://root@localhost:3306/hiveweb_test_ci",
        "mysql://root@[::1]:3306/hiveweb_test_worker_1",
    ] {
        common::validate_test_database_url(url)
            .unwrap_or_else(|error| panic!("expected `{url}` to be accepted: {error}"));
    }
}

#[test]
fn rejects_non_loopback_hosts() {
    let error = common::validate_test_database_url(
        "mysql://root:password@db.example.com:3306/hiveweb_test",
    )
    .expect_err("non-loopback database host must be rejected");
    assert!(error.to_string().contains("loopback"));
}

#[test]
fn rejects_non_test_or_missing_database_names() {
    for url in [
        "mysql://root@localhost:3306/hiveweb",
        "mysql://root@localhost:3306/mysql",
        "mysql://root@localhost:3306",
    ] {
        let error = common::validate_test_database_url(url)
            .expect_err("normal or missing database name must be rejected");
        assert!(
            error.to_string().contains("hiveweb_test"),
            "unexpected error for `{url}`: {error}"
        );
    }
}

#[test]
fn rejects_invalid_mysql_urls() {
    let error = common::validate_test_database_url("not-a-mysql-url")
        .expect_err("invalid database URL must be rejected");
    assert!(error.to_string().contains("invalid TEST_DATABASE_URL"));
}

#[test]
fn integration_harness_provisions_legacy_external_game_fixture() {
    let harness = workspace_file("crates/hiveweb/tests/common/mod.rs");
    let endpoint_tests = workspace_file("crates/hiveweb/tests/it_recommended_game_top.rs");
    let compact_endpoint_tests: String = endpoint_tests
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let workflow = workspace_file(".github/workflows/ci.yml");

    assert!(
        harness.contains("pub async fn test_external_pool()"),
        "the harness must expose a validated, independent external test pool"
    );
    assert!(
        harness.contains("pub async fn test_app_with_external_pool("),
        "the harness must allow only the legacy endpoint test to inject an external pool"
    );
    assert!(
        compact_endpoint_tests.contains("prepare_recommended_game_top_fixture(&ext_pool).await"),
        "the valid legacy endpoint test must provision its isolated external schema"
    );
    assert!(
        workflow.contains("TEST_EXTERNAL_DATABASE_URL:")
            && workflow.contains("hiveweb_test_external"),
        "CI must supply a distinct disposable external test database"
    );
    for table in [
        "recommended_games",
        "cc_logic_game",
        "cc_logic_game_wide",
        "cc_game",
        "cc_game_platform",
        "cc_computer_info",
        "cc_logic_game_version",
        "cc_promotion_channel",
        "cc_logic_game_exclude",
        "cc_logic_game_blacklist",
    ] {
        assert!(
            endpoint_tests.contains(&format!("CREATE TABLE IF NOT EXISTS {table}")),
            "missing disposable fixture table `{table}`"
        );
    }
}

#[test]
fn accepts_only_loopback_disposable_external_test_databases() {
    common::validate_test_external_database_url(
        "mysql://root@127.0.0.1:3306/hiveweb_test_external",
    )
    .expect("loopback disposable external database must be accepted");

    for url in [
        "mysql://root@db.example.com:3306/hiveweb_test_external",
        "mysql://root@localhost:3306/hiveweb_test",
        "mysql://root@localhost:3306/hiveweb_test_ci",
        "mysql://root@localhost:3306/external_games",
        "mysql://root@localhost:3306",
    ] {
        let error = common::validate_test_external_database_url(url)
            .expect_err("unsafe external database URL must be rejected");
        assert!(
            error.to_string().contains("TEST_EXTERNAL_DATABASE_URL"),
            "error must identify the guarded variable for `{url}`: {error}"
        );
    }
}

#[test]
fn requires_independent_main_and_external_test_databases() {
    common::validate_independent_test_database_urls(
        "mysql://root@127.0.0.1:3306/hiveweb_test",
        "mysql://root@127.0.0.1:3306/hiveweb_test_external",
    )
    .expect("distinct disposable test databases must be accepted");

    let shared_url = "mysql://root@127.0.0.1:3306/hiveweb_test_external";
    let error = common::validate_independent_test_database_urls(shared_url, shared_url)
        .expect_err("the main and external fixtures must not share a database");
    assert!(
        error.to_string().contains("different database"),
        "unexpected shared-database error: {error}"
    );
}

#[test]
fn developer_binaries_remain_available_without_features() {
    let manifest = workspace_file("crates/hiveweb/Cargo.toml");
    let workflow = workspace_file(".github/workflows/ci.yml");

    assert!(
        !manifest.contains("required-features"),
        "seed, seed-bench, and api-test must retain their existing feature-free CLI"
    );
    assert!(
        !workflow.contains("--features dev-tools") && !workflow.contains("--features bench-tools"),
        "CI must validate the existing binary entry points without feature-only aliases"
    );
}

#[test]
fn unrelated_workflow_placeholder_test_is_preserved() {
    let tests = workspace_file("crates/hiveweb/tests/it_agent_hook.rs");
    assert!(
        tests.contains("t017_workflow_hook_executes_on_after_agent_end"),
        "the ignored T017 workflow placeholder does not use the removed admin-chat API"
    );
}

#[test]
fn integration_images_are_version_pinned() {
    let workflow = workspace_file(".github/workflows/ci.yml");

    assert!(
        workflow.contains("minio/minio:RELEASE.2025-07-18T21-56-31Z"),
        "MinIO server image must use the last official immutable release tag"
    );
    assert!(
        workflow.contains("minio/mc:RELEASE.2025-08-13T08-35-41Z"),
        "MinIO client image must use an immutable release tag"
    );
    assert!(
        !workflow.contains("minio/minio:latest") && !workflow.contains("minio/mc:latest"),
        "quality CI must not depend on mutable MinIO image tags"
    );
}

#[test]
fn quality_job_installs_required_linux_packages_before_clippy() {
    let workflow = workspace_file(".github/workflows/ci.yml");
    let quality_job = workflow_job(&workflow, "quality", Some("hiveweb-integration"));
    let install = workflow_step_containing(quality_job, "sudo apt-get install -y");
    let install_position = quality_job
        .find("sudo apt-get install -y")
        .expect("quality CI must install GPUI system dependencies");
    let clippy = quality_job
        .find("cargo clippy --locked --workspace --all-targets -- -D warnings")
        .expect("quality CI must preserve strict Clippy");

    for package in ["libfontconfig-dev", "libxkbcommon-x11-dev"] {
        assert!(
            install
                .split_ascii_whitespace()
                .any(|token| token == package),
            "quality CI system dependency installation must include `{package}`"
        );
    }
    assert!(
        install.contains("pkg-config --exists fontconfig xkbcommon xkbcommon-x11"),
        "quality CI must verify the GPUI pkg-config modules after installation"
    );
    assert!(
        install_position < clippy,
        "GPUI system dependencies must be installed before cargo clippy"
    );
}

#[test]
fn rust_jobs_limit_runner_disk_usage_before_compilation() {
    let workflow = workspace_file(".github/workflows/ci.yml");
    let jobs = workflow
        .find("\njobs:\n")
        .expect("CI workflow must define jobs");
    let global_configuration = &workflow[..jobs];

    for setting in [
        "CARGO_INCREMENTAL: \"0\"",
        "CARGO_PROFILE_DEV_DEBUG: \"0\"",
        "CARGO_PROFILE_TEST_DEBUG: \"0\"",
    ] {
        assert!(
            global_configuration
                .lines()
                .any(|line| line.trim() == setting),
            "`{setting}` must apply to every Rust CI job"
        );
    }

    for (job_name, next_job, first_compilation_command) in [
        (
            "quality",
            Some("hiveweb-integration"),
            "cargo clippy --locked --workspace --all-targets -- -D warnings",
        ),
        (
            "hiveweb-integration",
            None,
            "cargo build --locked -p hiveweb --bin migrate",
        ),
    ] {
        let job = workflow_job(&workflow, job_name, next_job);
        let compilation = job.find(first_compilation_command).unwrap_or_else(|| {
            panic!("CI job `{job_name}` must preserve `{first_compilation_command}`")
        });
        let before_compilation = &job[..compilation];
        let cleanup = workflow_step_containing(before_compilation, "sudo rm -rf");
        for path in [
            "/usr/local/lib/android",
            "/usr/share/dotnet",
            "/opt/ghc",
            "/opt/hostedtoolcache/CodeQL",
        ] {
            assert!(
                cleanup.contains(path),
                "CI job `{job_name}` must reclaim `{path}` before compilation"
            );
        }
        assert!(
            cleanup.contains("docker image prune --all --force"),
            "CI job `{job_name}` must prune unused Docker images before compilation"
        );

        let cache = workflow_step_containing(job, "uses: actions/cache@");
        for source_cache in ["~/.cargo/registry", "~/.cargo/git"] {
            assert!(
                cache.lines().any(|line| line.trim() == source_cache),
                "CI job `{job_name}` must retain the `{source_cache}` source cache"
            );
        }
        assert!(
            !cache.lines().any(|line| line.trim() == "target"),
            "CI job `{job_name}` must not cache target artifacts on hosted runners"
        );
        if job_name == "hiveweb-integration" {
            assert!(
                !cache.contains("restore-keys:"),
                "HiveWeb integration must not use the former broad Cargo cache fallback"
            );
        }
    }
}
