mod common;

use std::path::Path;

fn workspace_file(relative_path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative_path);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
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
