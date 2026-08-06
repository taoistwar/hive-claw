fn bin_manifest<'a>(manifest: &'a toml::Value, name: &str) -> &'a toml::Value {
    manifest
        .get("bin")
        .and_then(toml::Value::as_array)
        .and_then(|bins| {
            bins.iter()
                .find(|bin| bin.get("name").and_then(toml::Value::as_str) == Some(name))
        })
        .unwrap_or_else(|| panic!("missing [[bin]] entry for {name}"))
}

#[test]
fn developer_binaries_are_feature_gated() {
    let manifest: toml::Value = include_str!("../Cargo.toml")
        .parse()
        .expect("hiveweb Cargo.toml must parse");

    for name in ["api-test", "seed"] {
        let required_features = bin_manifest(&manifest, name)
            .get("required-features")
            .and_then(toml::Value::as_array)
            .expect("developer binary must declare required-features");
        assert!(
            required_features
                .iter()
                .any(|feature| feature.as_str() == Some("dev-tools")),
            "{name} must require the dev-tools feature"
        );
    }

    let required_features = bin_manifest(&manifest, "seed-bench")
        .get("required-features")
        .and_then(toml::Value::as_array)
        .expect("seed-bench must declare required-features");
    assert!(
        required_features
            .iter()
            .any(|feature| feature.as_str() == Some("bench-tools")),
        "seed-bench must require the independent bench-tools feature"
    );
}

#[test]
fn audit_retention_requires_an_independent_database_identity() {
    let source = include_str!("../src/bin/audit_retention.rs");

    assert!(source.contains("AUDIT_RETENTION_DATABASE_URL"));
    assert!(!source.contains("std::env::var(\"DATABASE_URL\")"));
    assert!(
        source.contains("anyhow::bail!(\"runtime audit retention cleanup failed; exiting\")"),
        "cleanup failure must terminate the worker instead of being swallowed forever"
    );
    assert!(
        source
            .matches("error_kind = \"runtime_audit_retention_failed\"")
            .count()
            >= 2,
        "both connection and cleanup failures must emit the static retention error kind"
    );
}

#[test]
fn cli_sources_do_not_emit_or_accept_plaintext_passwords_and_signatures() {
    let api_test = include_str!("../src/bin/api_test.rs");
    for forbidden in [
        "println!(\"secret=",
        "sign data=",
        "sign hash=",
        "%sign_body",
        "println!(\"url:",
        "body:{}",
        "display_body",
        "to_string_pretty",
        ".send().await?",
        "tracing::",
        "tracing_subscriber",
    ] {
        assert!(
            !api_test.contains(forbidden),
            "api-test still contains `{forbidden}`"
        );
    }
    assert!(
        api_test.contains("response_summary(&method_upper, status, &response_body)"),
        "api-test must render only the bounded response summary"
    );
    assert!(
        api_test.contains("API test request failed"),
        "request errors must not expose a signed URL"
    );
    assert!(api_test.contains("--body-stdin"));
    assert!(api_test.contains("--body-file"));
    assert!(
        api_test.contains("positional request bodies are deprecated"),
        "argv body compatibility must carry a deprecation warning"
    );
    assert!(
        api_test.contains("read_private_file"),
        "api-test body files must use the shared descriptor-safe reader"
    );

    let create_admin = include_str!("../src/bin/create_super_admin.rs");
    let create_admin_production = create_admin
        .split("#[cfg(test)]")
        .next()
        .expect("create-super-admin production source");
    assert!(!create_admin.contains("\"--password\" =>"));
    assert!(!create_admin.contains("println!(\"  Password: {}\", password)"));
    assert!(!create_admin.contains("println!(\"Phone: {}\", cli.phone)"));
    assert!(!create_admin_production.contains("ON DUPLICATE KEY"));
    assert!(!create_admin_production.contains("bcrypt::hash"));
    assert!(create_admin_production.contains("utils::password::hash_password"));
    assert!(create_admin_production.contains("is_unique_violation"));
    assert!(
        create_admin.contains("--password-stdin") || create_admin.contains("--password-file"),
        "create-super-admin must use stdin or a secret file"
    );
    assert!(
        create_admin.contains("read_private_file"),
        "create-super-admin must use the shared descriptor-safe reader"
    );

    let seed = include_str!("../src/bin/seed.rs");
    assert!(!seed.contains("admin123"));
    assert!(!seed.contains("let phone = \""));
    assert!(!seed.contains("println!(\"  Password:"));
    assert!(!seed.contains("println!(\"  Phone:"));
    assert!(!seed.contains("bcrypt::hash"));
    assert!(seed.contains("utils::password::hash_password"));
    assert!(seed.contains("HIVEWEB_DEV_SEED_ADMIN_PASSWORD_FILE"));
    assert!(seed.contains("HIVEWEB_DEV_SEED_ADMIN_PHONE"));
    assert!(
        seed.contains("read_private_file"),
        "seed must use the shared descriptor-safe reader"
    );

    let secret_input = include_str!("../src/bin/support/secret_input.rs");
    for required in [
        "OpenOptionsExt",
        "libc::O_NOFOLLOW",
        ".metadata()",
        ".file_type().is_file()",
        ".permissions().mode() & 0o077 == 0",
        ".uid()",
        "libc::geteuid()",
        ".nlink() == 1",
    ] {
        assert!(
            secret_input.contains(required),
            "descriptor-safe file reader is missing `{required}`"
        );
    }
    assert!(
        !secret_input.contains("symlink_metadata"),
        "file validation must not inspect the path before opening it"
    );

    let seed_bench = include_str!("../src/bin/seed_bench.rs");
    assert!(seed_bench.contains("--confirm-destructive"));
    assert!(seed_bench.contains("get_database()"));
    assert!(seed_bench.contains("_test"));
    assert!(seed_bench.contains("_bench"));
    assert!(!seed_bench.contains("bench-pass-1"));
}

#[test]
fn chat_retention_fails_closed_on_invalid_configuration_and_runtime_errors() {
    let source = include_str!("../src/bin/chat_retention.rs");

    assert!(source.contains("parse_positive"));
    assert!(
        source
            .matches("error_kind = \"chat_retention_failed\"")
            .count()
            >= 2,
        "both connection and cleanup failures must emit the static error kind"
    );
    assert!(
        source.contains("chat retention cleanup failed; exiting"),
        "cleanup errors must terminate the worker"
    );
}

#[test]
fn shared_password_hashing_errors_are_static() {
    let source = include_str!("../src/utils/password.rs");

    assert!(
        !source.contains("Failed to hash password:"),
        "bcrypt implementation details must not be copied into an AppError"
    );
    assert!(
        !source.contains("Failed to verify password:"),
        "bcrypt verification details must not be copied into an AppError"
    );
}

#[test]
fn operator_docs_define_least_privilege_accounts_and_safe_cli_usage() {
    let env_example = include_str!("../.env.example");
    let readme = include_str!("../README.md");
    let security = include_str!("../../../specs/004-agent-runtime/SECURITY.md");
    let prod_install = include_str!("../../../docs/prod/install.md");
    let admin_deployment = include_str!("../../../specs/003-admin-center/DEPLOYMENT.md");
    let admin_quickstart = include_str!("../../../specs/003-admin-center/quickstart.md");
    let admin_plan = include_str!("../../../specs/003-admin-center/plan.md");

    assert!(env_example.contains("AUDIT_RETENTION_DATABASE_URL="));
    assert!(!readme.contains("--password adminpass"));
    assert!(readme.contains("--features dev-tools --bin api-test"));
    assert!(readme.contains("--features dev-tools --bin seed"));
    assert!(readme.contains("--features bench-tools --bin seed-bench"));
    assert!(readme.contains("--body-stdin"));
    assert!(readme.contains("--body-file"));
    assert!(readme.contains("HIVEWEB_DEV_SEED_ADMIN_PASSWORD_FILE"));
    assert!(readme.contains("HIVEWEB_DEV_SEED_ADMIN_PHONE"));
    assert!(readme.contains("never prints the response body"));
    assert!(env_example.contains("HIVEWEB_DEV_SEED_ADMIN_PHONE="));
    assert!(readme.contains("INSERT-only"));
    assert!(readme.contains("authenticated password-change flow"));

    for (name, document) in [
        ("production install", prod_install),
        ("003 deployment", admin_deployment),
        ("003 quickstart", admin_quickstart),
        ("003 plan", admin_plan),
    ] {
        assert!(
            !document.contains("--password "),
            "{name} still recommends a plaintext argv password"
        );
        assert!(
            document.contains("--password-stdin"),
            "{name} must show the safe bootstrap password source"
        );
    }
    assert!(!prod_install.contains("重复手机号只更新昵称"));
    assert!(prod_install.contains("重复手机号会以非零状态失败"));

    let ci = include_str!("../../../.github/workflows/ci.yml");
    assert!(ci.contains("test --locked -p hiveweb --features dev-tools --bin api-test --bin seed"));
    assert!(ci.contains("test --locked -p hiveweb --features bench-tools --bin seed-bench"));

    for required in [
        "GRANT INSERT, SELECT ON hiveweb.runtime_audit_logs",
        "REVOKE UPDATE, DELETE ON hiveweb.runtime_audit_logs",
        "GRANT DELETE ON hiveweb.runtime_audit_logs",
        "AUDIT_RETENTION_DATABASE_URL",
    ] {
        assert!(
            security.contains(required),
            "SECURITY.md is missing `{required}`"
        );
    }
}
