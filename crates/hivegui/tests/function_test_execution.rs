use std::{
    path::Path,
    time::{Duration, Instant},
};

use hive_builtins::{format_template, json_parse, json_stringify, text_regex_match};
use serde_json::Value;

use hivegui::{
    datasource::{
        entity_store::{Capability, Function},
        migrations::{MigrationOptions, migrate_to_current},
    },
    plugin::plugin_store::{PluginArtifactInput, PluginMetadata, PluginStore},
    runtime::{
        FUNCTION_TEST_TIMEOUT, FunctionTestExecutor, PluginExecutor, capability_matches_filter,
        format_test_output, resolve_test_capabilities,
    },
};

mod support;

#[path = "support/plugin_wat.rs"]
mod plugin_wat;

use support::TestWorkspace;

fn custom_function(plugin_id: Option<i64>, plugin_export: Option<&str>) -> Function {
    Function {
        id: 7,
        identifier: "weather-lookup".into(),
        name: "weather-lookup".into(),
        description: None,
        kind: "custom".to_string(),
        input_schema: r#"{"type":"object"}"#.into(),
        output_schema: "{}".into(),
        plugin_id,
        plugin_export: plugin_export.map(str::to_owned),
        category_id: None,
        required_capabilities: None,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

fn builtin_function_test_case(identifier: &str) -> (&'static str, &'static str, Value, Value) {
    // Dotted legacy identifiers share the same schemas as their underscore
    // counterparts; the executor is responsible for rejecting the dotted form.
    let normalized = identifier.replace('.', "_");
    match normalized.as_str() {
        "format_template" => (
            format_template::FORMAT_TEMPLATE_INPUT_SCHEMA,
            format_template::FORMAT_TEMPLATE_OUTPUT_SCHEMA,
            serde_json::json!({"template": "Hello {name}", "vars": {"name": "Alice"}}),
            serde_json::json!("Hello Alice"),
        ),
        "json_parse" => (
            json_parse::JSON_PARSE_INPUT_SCHEMA,
            json_parse::JSON_PARSE_OUTPUT_SCHEMA,
            serde_json::json!({"text": "{\"message\":\"ok\",\"n\":1}"}),
            serde_json::json!({"message": "ok", "n": 1}),
        ),
        "json_stringify" => (
            json_stringify::JSON_STRINGIFY_INPUT_SCHEMA,
            json_stringify::JSON_STRINGIFY_OUTPUT_SCHEMA,
            serde_json::json!({"value": {"count": 1}, "pretty": false}),
            serde_json::json!("{\"count\":1}"),
        ),
        "text_regex_match" => (
            text_regex_match::TEXT_REGEX_MATCH_INPUT_SCHEMA,
            text_regex_match::TEXT_REGEX_MATCH_OUTPUT_SCHEMA,
            serde_json::json!({"text": "a1,b2,c3", "pattern": "([a-z])(\\d)", "all": true}),
            serde_json::json!({"matches": [
                {"full": "a1", "groups": ["a", "1"]},
                {"full": "b2", "groups": ["b", "2"]},
                {"full": "c3", "groups": ["c", "3"]},
            ]}),
        ),
        _ => panic!("unsupported builtin identifier {identifier}"),
    }
}

fn builtin_function(identifier: &str) -> Function {
    let mut function = custom_function(None, None);
    function.kind = "builtin".to_string();
    function.identifier = identifier.into();
    function.name = identifier.into();
    function.plugin_id = None;
    function.plugin_export = None;
    let (input_schema, output_schema, _, _) = builtin_function_test_case(identifier);
    function.input_schema = input_schema.to_string();
    function.output_schema = output_schema.to_string();
    function
}

fn custom_plugin_function(
    plugin_id: i64,
    plugin_export: &str,
    required_capabilities: Option<&str>,
) -> Function {
    let mut function = custom_function(Some(plugin_id), Some(plugin_export));
    function.identifier = "smoke_echo".into();
    function.name = "smoke_echo".into();
    function.input_schema = r#"{"type":"string"}"#.into();
    function.output_schema =
        r#"{"type":"object","properties":{"echo":{"type":"string"},"logged":{"type":"boolean"}},"required":["echo","logged"]}"#.into();
    function.required_capabilities = required_capabilities.map(str::to_string);
    function
}

/// Identifier/version the shared smoke fixture is staged under. Matches the
/// unified `{identifier}/{version}/{id}/plugin.wasm` layout the executor reads.
const SMOKE_PLUGIN_IDENTIFIER: &str = "smoke";
const SMOKE_PLUGIN_VERSION: &str = "1.0.0";
const SHARED_SMOKE_WASM: &[u8] = include_bytes!("fixtures/plugins/shared-smoke/plugin.wasm");
const SHARED_SMOKE_MANIFEST: &str = include_str!("fixtures/plugins/shared-smoke/manifest.json");
const SHARED_SMOKE_CAPABILITIES: &[&str] = &[
    "fs.read",
    "fs.write",
    "log.emit",
    "network.http",
    "time.now",
];

async fn empty_managed_store() -> (TestWorkspace, sqlx::SqlitePool) {
    let workspace = TestWorkspace::new().expect("create isolated verified Plugin workspace");
    migrate_to_current(MigrationOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("migrate verified Plugin workspace");
    let pool = workspace.sqlite_pool().await.expect("open SQLite pool");

    (workspace, pool)
}

async fn installed_verified_smoke_plugin() -> (TestWorkspace, sqlx::SqlitePool, i64) {
    let (workspace, pool) = empty_managed_store().await;
    for capability in SHARED_SMOKE_CAPABILITIES {
        Capability::create(
            &pool,
            (*capability).to_string(),
            format!("Test-local {capability} fixture capability"),
            false,
            None,
        )
        .await
        .unwrap_or_else(|error| panic!("register fixture Capability {capability}: {error}"));
    }

    let required_capabilities = SHARED_SMOKE_CAPABILITIES
        .iter()
        .map(|capability| (*capability).to_string())
        .collect::<Vec<_>>();
    let store = PluginStore::new(pool.clone(), workspace.plugin_root())
        .expect("open verified Plugin store");
    let plugin = store
        .install_with_metadata(
            PluginArtifactInput::new(
                SMOKE_PLUGIN_IDENTIFIER,
                SMOKE_PLUGIN_VERSION,
                SHARED_SMOKE_WASM,
            )
            .expect("build frozen ABI-v1 Plugin input"),
            PluginMetadata::new(
                "Frozen shared smoke Plugin",
                None,
                Some(SHARED_SMOKE_MANIFEST.to_string()),
                "extism",
                serde_json::to_string(&required_capabilities)
                    .expect("serialize fixture Capability list"),
                "{}",
            ),
        )
        .await
        .expect("install frozen ABI-v1 Plugin through the real store");

    (workspace, pool, plugin.id())
}

fn managed_executor(plugin_root: &Path, pool: &sqlx::SqlitePool) -> FunctionTestExecutor {
    FunctionTestExecutor::new(plugin_root.to_path_buf(), pool.clone())
}

async fn execute_managed(
    executor: &FunctionTestExecutor,
    function: &Function,
    input: Value,
) -> Result<String, String> {
    executor.execute(function, input).await
}

async fn execute_managed_with_capabilities(
    executor: &FunctionTestExecutor,
    function: &Function,
    input: Value,
    capability_snapshot: Vec<String>,
) -> Result<String, String> {
    executor
        .execute_with_capabilities(function, input, capability_snapshot)
        .await
}

#[test]
fn function_tests_use_the_runtime_limit_from_the_agent_runtime_spec() {
    assert_eq!(FUNCTION_TEST_TIMEOUT, Duration::from_secs(30));
}

#[test]
fn test_capability_defaults_use_exact_names_and_drop_deleted_entries() {
    let available = vec!["net".to_string(), "network.http".to_string()];
    assert_eq!(
        resolve_test_capabilities(Some(r#"["network.http"]"#), &available),
        ["network.http".to_string()].into_iter().collect()
    );

    assert!(
        resolve_test_capabilities(Some(r#"["net"]"#), &["network.http".to_string()]).is_empty(),
        "a deleted capability must not remain selected from the function's stored defaults"
    );
}

#[test]
fn capability_filter_matches_names_and_descriptions_case_insensitively() {
    assert!(capability_matches_filter(
        "network.http",
        "HTTP/HTTPS access",
        "NETWORK"
    ));
    assert!(capability_matches_filter(
        "network.http",
        "HTTP/HTTPS access",
        "https"
    ));
    assert!(!capability_matches_filter(
        "network.http",
        "HTTP/HTTPS access",
        "database"
    ));
    assert!(capability_matches_filter("network.http", "", ""));
}

#[test]
fn json_test_output_is_pretty_printed_and_plain_text_is_preserved() {
    assert_eq!(
        format_test_output(r#"{"city":"beijing","temp_c":17}"#),
        "{\n  \"city\": \"beijing\",\n  \"temp_c\": 17\n}"
    );
    assert_eq!(format_test_output("plain text result"), "plain text result");
}

fn record_stable_error_result(
    entrypoint: &str,
    expected_kind: &str,
    result: Result<String, String>,
    violations: &mut Vec<String>,
) {
    match result {
        Err(error) if error == expected_kind => {}
        Err(error) => violations.push(format!(
            "{entrypoint} returned {error:?}; expected exact stable kind {expected_kind:?}"
        )),
        Ok(output) => violations.push(format!(
            "{entrypoint} succeeded with {output:?}; expected exact stable kind {expected_kind:?}"
        )),
    }
}

#[tokio::test]
async fn builtin_functions_execute_and_validate_schema_and_output() {
    let (workspace, pool) = empty_managed_store().await;
    let executor = managed_executor(workspace.plugin_root(), &pool);

    for identifier in [
        "format_template",
        "json_parse",
        "json_stringify",
        "text_regex_match",
    ] {
        let (input_schema, output_schema, input, expected_output) =
            builtin_function_test_case(identifier);
        let function = builtin_function(identifier);
        let normalized_input_schema: Value =
            serde_json::from_str(input_schema).expect("builtin input schema must be valid json");
        let normalized_output_schema: Value =
            serde_json::from_str(output_schema).expect("builtin output schema must be valid json");

        assert_eq!(
            serde_json::from_str::<Value>(&function.input_schema)
                .expect("builtin function input schema should be valid json"),
            normalized_input_schema
        );
        assert_eq!(
            serde_json::from_str::<Value>(&function.output_schema)
                .expect("builtin function output schema should be valid json"),
            normalized_output_schema
        );

        let output = execute_managed(&executor, &function, input.clone())
            .await
            .expect("managed execute must run every underscore Builtin")
            .as_str()
            .parse::<Value>()
            .unwrap_or_else(|error| {
                panic!("builtin output must be JSON for {identifier}: {error}")
            });
        assert_eq!(output, expected_output);

        let output = execute_managed_with_capabilities(&executor, &function, input, Vec::new())
            .await
            .expect("managed explicit-capability entry must run every underscore Builtin")
            .as_str()
            .parse::<Value>()
            .unwrap_or_else(|error| {
                panic!("builtin output must be JSON for {identifier}: {error}")
            });
        assert_eq!(output, expected_output);
    }
}

#[tokio::test]
async fn dotted_builtin_names_are_not_found_from_every_public_entry() {
    let (workspace, pool) = empty_managed_store().await;
    let executor = managed_executor(workspace.plugin_root(), &pool);
    let mut violations = Vec::new();

    for identifier in [
        "format.template",
        "json.parse",
        "json.stringify",
        "text.regex_match",
    ] {
        let function = builtin_function(identifier);
        let input = serde_json::json!({});

        record_stable_error_result(
            &format!("execute({identifier})"),
            "not_found",
            execute_managed(&executor, &function, input.clone()).await,
            &mut violations,
        );
        record_stable_error_result(
            &format!("execute_with_capabilities({identifier})"),
            "not_found",
            execute_managed_with_capabilities(&executor, &function, input, Vec::new()).await,
            &mut violations,
        );
    }

    assert!(
        violations.is_empty(),
        "all dotted Builtin names must return only stable not_found from both managed entries: {violations:#?}"
    );
}

#[tokio::test]
async fn custom_plugin_function_executes_with_explicit_capability_snapshot() {
    let (workspace, pool, plugin_id) = installed_verified_smoke_plugin().await;
    let executor = managed_executor(workspace.plugin_root(), &pool);

    let expected_input_schema: Value = serde_json::from_str(r#"{"type":"string"}"#).unwrap();
    let expected_output_schema: Value = serde_json::json!({
        "type": "object",
        "properties": {
            "echo": { "type": "string" },
            "logged": { "type": "boolean" },
        },
        "required": ["echo", "logged"],
    });

    let function = custom_plugin_function(plugin_id, "echo", Some(r#"["log.emit"]"#));

    assert_eq!(
        serde_json::from_str::<Value>(&function.input_schema)
            .expect("custom plugin input schema should be valid json"),
        expected_input_schema
    );
    assert_eq!(
        serde_json::from_str::<Value>(&function.output_schema)
            .expect("custom plugin output schema should be valid json"),
        expected_output_schema
    );

    let input = serde_json::json!("hello");
    let expected_output = serde_json::json!({
        "echo": "hello",
        "logged": true,
    });

    let output = serde_json::from_str::<Value>(
        &execute_managed_with_capabilities(
            &executor,
            &function,
            input,
            vec!["log.emit".to_string()],
        )
        .await
        .expect("managed Custom Function should execute with an explicit snapshot"),
    )
    .expect("custom plugin output should be JSON");
    assert_eq!(output, expected_output);
}

#[tokio::test]
async fn custom_plugin_function_fails_when_needed_capability_is_not_granted() {
    let (workspace, pool, plugin_id) = installed_verified_smoke_plugin().await;
    let executor = managed_executor(workspace.plugin_root(), &pool);
    let function = custom_plugin_function(plugin_id, "echo", Some(r#"["log.emit"]"#));
    let input = serde_json::json!("hello");
    let mut violations = Vec::new();

    record_stable_error_result(
        "managed execute with its empty default Capability snapshot",
        "capability_denied",
        execute_managed(&executor, &function, input.clone()).await,
        &mut violations,
    );
    record_stable_error_result(
        "managed execute_with_capabilities with an explicit empty snapshot",
        "capability_denied",
        execute_managed_with_capabilities(&executor, &function, input, Vec::new()).await,
        &mut violations,
    );

    assert!(
        violations.is_empty(),
        "both implicit-empty and explicit-empty Capability snapshots must return exact capability_denied: {violations:#?}"
    );
}

#[tokio::test]
async fn real_custom_plugin_input_schema_violation_fails_closed_in_every_public_entry() {
    let (workspace, pool, plugin_id) = installed_verified_smoke_plugin().await;
    let executor = managed_executor(workspace.plugin_root(), &pool);
    let mut function = custom_plugin_function(plugin_id, "echo", Some(r#"["log.emit"]"#));
    function.input_schema = r#"{"type":"object"}"#.into();
    let input = serde_json::json!("guest-would-accept-this-string");
    let mut violations = Vec::new();

    record_schema_violation_result(
        "managed execute(real Custom Plugin)",
        "input_schema",
        execute_managed(&executor, &function, input.clone()).await,
        &mut violations,
    );
    record_schema_violation_result(
        "managed execute_with_capabilities(real Custom Plugin)",
        "input_schema",
        execute_managed_with_capabilities(
            &executor,
            &function,
            input,
            vec!["log.emit".to_string()],
        )
        .await,
        &mut violations,
    );

    assert!(
        violations.is_empty(),
        "real ABI-v1 Custom Plugin input must be rejected before guest execution when it violates Function.input_schema: {violations:#?}"
    );
}

#[tokio::test]
async fn real_custom_plugin_guest_output_schema_violation_fails_closed_in_every_public_entry() {
    let (workspace, pool, plugin_id) = installed_verified_smoke_plugin().await;
    let executor = managed_executor(workspace.plugin_root(), &pool);
    let mut function = custom_plugin_function(plugin_id, "echo", Some(r#"["log.emit"]"#));
    function.output_schema = r#"{"type":"array"}"#.into();
    let input = serde_json::json!("guest-produces-an-object");
    let mut violations = Vec::new();

    record_schema_violation_result(
        "managed execute_with_capabilities(real Custom Plugin)",
        "output_schema",
        execute_managed_with_capabilities(
            &executor,
            &function,
            input,
            vec!["log.emit".to_string()],
        )
        .await,
        &mut violations,
    );

    assert!(
        violations.is_empty(),
        "real ABI-v1 Custom Plugin guest output must return exact output_schema_mismatch after explicit-capability execution: {violations:#?}"
    );
}

#[tokio::test]
async fn custom_function_precondition_errors_return_instead_of_stalling() {
    let (workspace, pool) = empty_managed_store().await;
    let executor = managed_executor(workspace.plugin_root(), &pool);

    let missing_plugin = tokio::time::timeout(
        Duration::from_secs(1),
        execute_managed(
            &executor,
            &custom_function(None, Some("lookup")),
            serde_json::json!({"city": "beijing"}),
        ),
    )
    .await
    .expect("missing plugin validation must finish")
    .expect_err("missing plugin must fail");
    assert_eq!(missing_plugin, "函数未关联插件");

    let missing_export = execute_managed(
        &executor,
        &custom_function(Some(1), None),
        serde_json::json!({"city": "beijing"}),
    )
    .await
    .expect_err("missing export must fail");
    assert_eq!(missing_export, "函数未指定插件导出函数名");

    let missing_plugin_record = execute_managed(
        &executor,
        &custom_function(Some(1), Some("lookup")),
        serde_json::json!({"city": "beijing"}),
    )
    .await
    .expect_err("missing managed Plugin record must fail");
    assert_eq!(missing_plugin_record, "plugin_missing");
}

fn record_schema_violation_result(
    entrypoint: &str,
    schema_field: &str,
    result: Result<String, String>,
    violations: &mut Vec<String>,
) {
    let expected_kind = match schema_field {
        "input_schema" => "input_schema_mismatch",
        "output_schema" => "output_schema_mismatch",
        other => panic!("unsupported schema contract field {other}"),
    };
    match result {
        Err(error) if error == expected_kind => {}
        Err(error) => violations.push(format!(
            "{entrypoint} returned {error:?}; expected exact stable kind {expected_kind:?}"
        )),
        Ok(output) => violations.push(format!(
            "{entrypoint} accepted a value that violates {schema_field}: {output}; expected exact stable kind {expected_kind:?}"
        )),
    }
}

#[tokio::test]
async fn every_public_entry_rejects_input_that_violates_function_schema() {
    let (workspace, pool) = empty_managed_store().await;
    let executor = managed_executor(workspace.plugin_root(), &pool);
    let mut function = builtin_function("json_stringify");
    function.input_schema = r#"{"type":"array"}"#.into();
    let input = serde_json::json!({"value": {"message": "schema red"}});
    let mut violations = Vec::new();

    record_schema_violation_result(
        "execute",
        "input_schema",
        execute_managed(&executor, &function, input.clone()).await,
        &mut violations,
    );
    record_schema_violation_result(
        "execute_with_capabilities",
        "input_schema",
        execute_managed_with_capabilities(&executor, &function, input, Vec::new()).await,
        &mut violations,
    );

    assert!(
        violations.is_empty(),
        "all public Function execution entries must fail closed on input schema violations: {violations:#?}"
    );
}

#[tokio::test]
async fn every_public_entry_rejects_output_that_violates_function_schema() {
    let (workspace, pool) = empty_managed_store().await;
    let executor = managed_executor(workspace.plugin_root(), &pool);
    let mut function = builtin_function("json_stringify");
    function.output_schema = r#"{"type":"object"}"#.into();
    let input = serde_json::json!({"value": {"message": "schema red"}});
    let mut violations = Vec::new();

    record_schema_violation_result(
        "execute",
        "output_schema",
        execute_managed(&executor, &function, input.clone()).await,
        &mut violations,
    );
    record_schema_violation_result(
        "execute_with_capabilities",
        "output_schema",
        execute_managed_with_capabilities(&executor, &function, input, Vec::new()).await,
        &mut violations,
    );

    assert!(
        violations.is_empty(),
        "all public Function execution entries must fail closed on output schema violations: {violations:#?}"
    );
}

#[tokio::test]
async fn placeholder_function_is_non_executable_before_capability_or_plugin_resolution() {
    let temp_dir = tempfile::tempdir().expect("create temp directory");
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("open deliberately schema-free SQLite pool");
    let executor = managed_executor(temp_dir.path(), &pool);
    let mut function = custom_function(Some(i64::MAX), Some("missing_export"));
    function.kind = "placeholder".to_string();
    function.required_capabilities = Some(r#"[bad json"#.into());
    let input = serde_json::json!({"query": "prompt-only"});
    let mut violations = Vec::new();

    record_stable_error_result(
        "managed execute Placeholder before malformed Capability parsing",
        "function_not_executable",
        execute_managed(&executor, &function, input.clone()).await,
        &mut violations,
    );
    record_stable_error_result(
        "managed execute_with_capabilities Placeholder before Plugin lookup/guest creation",
        "function_not_executable",
        execute_managed_with_capabilities(
            &executor,
            &function,
            input,
            vec!["network.http".to_string()],
        )
        .await,
        &mut violations,
    );

    assert!(
        violations.is_empty(),
        "both managed entries must return exact function_not_executable before Capability parsing, schema-free DB Plugin lookup, or guest creation: {violations:#?}"
    );
}

#[tokio::test]
async fn lower_level_plugin_executor_timeout_is_exempt_from_function_executor_api_inventory() {
    // T074 lower-level sandbox coverage only. This direct PluginExecutor call
    // neither defines nor authorizes another public FunctionTestExecutor entry.
    let temp_dir = tempfile::tempdir().expect("create temp directory");
    let wasm_path = temp_dir.path().join("loop.wasm");
    let wasm = plugin_wat::compile_v1(
        "",
        r#"
          (func (export "loop_forever") (result i32)
            (loop $forever
              br $forever
            )
            i32.const 0
          )
        "#,
    );
    std::fs::write(&wasm_path, wasm).expect("write WASM fixture");

    let started = Instant::now();
    let error = PluginExecutor::execute_with_timeout(
        &wasm_path,
        "loop_forever",
        "{}",
        Duration::from_millis(50),
    )
    .await
    .expect_err("infinite-loop plugin must time out");

    assert!(error.contains("插件执行超时"), "unexpected error: {error}");
    assert!(started.elapsed() < Duration::from_secs(2));
}

fn public_function_test_executor_methods(source: &str) -> Vec<String> {
    let implementation = source
        .split_once("impl FunctionTestExecutor {")
        .map(|(_, implementation)| implementation)
        .expect("locate the single FunctionTestExecutor implementation");
    let mut methods = implementation
        .lines()
        .filter_map(|line| {
            let signature = line.trim_start().strip_prefix("pub ")?;
            let signature = signature.strip_prefix("async ").unwrap_or(signature);
            let signature = signature.strip_prefix("fn ")?;
            signature.split_once('(').map(|(name, _)| name.to_string())
        })
        .collect::<Vec<_>>();
    methods.sort();
    methods
}

fn public_function_test_executor_signature(source: &str, method: &str) -> String {
    let markers = [
        format!("pub fn {method}("),
        format!("pub async fn {method}("),
    ];
    let tail = markers
        .iter()
        .find_map(|marker| source.find(marker).map(|start| &source[start..]))
        .unwrap_or_else(|| panic!("locate public FunctionTestExecutor::{method} signature"));
    let signature = tail
        .split_once('{')
        .map(|(signature, _)| signature)
        .expect("public method signature must be followed by a body");
    signature.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn function_test_executor_public_api_is_exactly_two_managed_execution_entries() {
    let source = include_str!("../src/runtime/function_test_executor.rs");

    assert_eq!(
        public_function_test_executor_methods(source),
        ["execute", "execute_with_capabilities", "new"],
        "only the managed constructor and two instance execution entries may remain public; path-based/static helpers and execute_with_verification must be absent or private"
    );

    let constructor = public_function_test_executor_signature(source, "new");
    assert!(constructor.contains("plugin_root"));
    assert!(constructor.contains("pool"));

    for method in ["execute", "execute_with_capabilities"] {
        let signature = public_function_test_executor_signature(source, method);
        assert!(
            signature.contains("&self"),
            "FunctionTestExecutor::{method} must be an instance method: {signature}"
        );
        for forbidden in [
            "base_dir",
            "plugin_identifier",
            "plugin_version",
            "plugin_root",
            "pool:",
            "Path",
        ] {
            assert!(
                !signature.contains(forbidden),
                "FunctionTestExecutor::{method} must use constructor-owned managed state, not caller path/DB arguments; forbidden {forbidden:?} in {signature}"
            );
        }
    }
}

#[test]
fn function_view_routes_execution_errors_to_a_terminal_state() {
    let source = include_str!("../src/ui/function_view.rs");
    let run_test = source
        .split("fn run_test")
        .nth(1)
        .and_then(|tail| tail.split("impl Render").next())
        .expect("locate run_test implementation");

    assert!(run_test.contains("FunctionTestExecutor::new"));
    assert!(run_test.contains(".execute_with_capabilities("));
    assert!(
        !run_test.contains("execute_with_verification"),
        "the UI must not select a separate third entry or reopen a persisted artifact path"
    );
    assert!(run_test.contains("TestState::Error"));
    assert!(
        !run_test.contains("return Err("),
        "validation errors must not bypass the terminal state update"
    );
}
