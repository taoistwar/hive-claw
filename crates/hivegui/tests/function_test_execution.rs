use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use hive_builtins::{format_template, json_parse, json_stringify, text_regex_match};
use serde_json::Value;

use hivegui::{
    datasource::entity_store::Function,
    runtime::{
        FUNCTION_TEST_TIMEOUT, FunctionTestExecutor, PluginExecutor, capability_matches_filter,
        format_test_output, resolve_test_capabilities,
    },
};

fn custom_function(plugin_id: Option<i64>, plugin_export: Option<&str>) -> Function {
    Function {
        id: 7,
        identifier: "weather-lookup".into(),
        name: "weather-lookup".into(),
        description: None,
        kind: 2,
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
    function.kind = 1;
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

fn shared_smoke_plugin_wasm(base_dir: &Path, plugin_id: i64) -> PathBuf {
    let source_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/shared-smoke");
    let source = source_root.join("plugin.wasm");
    let target_dir = base_dir.join("plugins").join(plugin_id.to_string());

    fs::create_dir_all(&target_dir).expect("create shared smoke Plugin dir");
    let target = target_dir.join("plugin.wasm");
    fs::copy(&source, &target).expect("copy shared smoke Plugin fixture");
    target
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

#[tokio::test]
async fn builtin_functions_execute_and_validate_schema_and_output() {
    let temp_dir = tempfile::tempdir().expect("create temp directory");

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

        let output = FunctionTestExecutor::execute(&function, input.clone(), temp_dir.path())
            .await
            .expect("builtin must execute through underscore identifier")
            .as_str()
            .parse::<Value>()
            .unwrap_or_else(|error| {
                panic!("builtin output must be JSON for {identifier}: {error}")
            });
        assert_eq!(output, expected_output);

        let output = FunctionTestExecutor::execute_with_capabilities(
            &function,
            input,
            temp_dir.path(),
            Vec::new(),
        )
        .await
        .expect("builtin with explicit capability entrypoint should also execute")
        .as_str()
        .parse::<Value>()
        .unwrap_or_else(|error| panic!("builtin output must be JSON for {identifier}: {error}"));
        assert_eq!(output, expected_output);
    }
}

#[tokio::test]
async fn dotted_builtin_names_are_rejected_by_both_execution_entries() {
    let temp_dir = tempfile::tempdir().expect("create temp directory");

    for identifier in [
        "format.template",
        "json.parse",
        "json.stringify",
        "text.regex_match",
    ] {
        let function = builtin_function(identifier);
        let input = serde_json::json!({});

        let dotted_error = FunctionTestExecutor::execute(&function, input.clone(), temp_dir.path())
            .await
            .expect_err("dotted builtin identifier should be rejected in execute entrypoint");
        assert!(
            dotted_error.contains("Unknown pure builtin")
                || dotted_error.contains("not found")
                || dotted_error.contains("未找到")
        );

        let dotted_error = FunctionTestExecutor::execute_with_capabilities(
            &function,
            input,
            temp_dir.path(),
            Vec::new(),
        )
        .await
        .expect_err(
            "dotted builtin identifier should be rejected in explicit-capability entrypoint",
        );
        assert!(
            dotted_error.contains("Unknown pure builtin")
                || dotted_error.contains("not found")
                || dotted_error.contains("未找到")
        );
    }
}

#[tokio::test]
async fn custom_plugin_function_executes_via_both_execution_entries() {
    let temp_dir = tempfile::tempdir().expect("create temp directory");
    shared_smoke_plugin_wasm(temp_dir.path(), 1);

    let expected_input_schema: Value = serde_json::from_str(r#"{"type":"string"}"#).unwrap();
    let expected_output_schema: Value = serde_json::json!({
        "type": "object",
        "properties": {
            "echo": { "type": "string" },
            "logged": { "type": "boolean" },
        },
        "required": ["echo", "logged"],
    });

    let function = custom_plugin_function(1, "echo", Some(r#"["log.emit"]"#));

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
        &FunctionTestExecutor::execute(&function, input.clone(), temp_dir.path())
            .await
            .expect("custom plugin should execute in execute entrypoint"),
    )
    .expect("custom plugin output should be JSON");
    assert_eq!(output, expected_output);

    let output = serde_json::from_str::<Value>(
        &FunctionTestExecutor::execute_with_capabilities(
            &function,
            input,
            temp_dir.path(),
            vec!["log.emit".to_string()],
        )
        .await
        .expect("custom plugin should execute in explicit-capability entrypoint"),
    )
    .expect("custom plugin output should be JSON");
    assert_eq!(output, expected_output);
}

#[tokio::test]
async fn custom_plugin_function_fails_when_needed_capability_is_not_granted() {
    let temp_dir = tempfile::tempdir().expect("create temp directory");
    shared_smoke_plugin_wasm(temp_dir.path(), 1);

    let function = custom_plugin_function(1, "echo", Some(r#"["log.emit"]"#));
    let error = FunctionTestExecutor::execute_with_capabilities(
        &function,
        serde_json::json!("hello"),
        temp_dir.path(),
        Vec::new(),
    )
    .await
    .expect_err("missing Capability should fail plugin execution");

    assert!(
        error.contains("未授权")
            || error.contains("unauthorized")
            || error.contains("capability")
            || error.contains("permission")
    );
}

#[tokio::test]
async fn custom_function_precondition_errors_return_instead_of_stalling() {
    let temp_dir = tempfile::tempdir().expect("create temp directory");

    let missing_plugin = tokio::time::timeout(
        Duration::from_secs(1),
        FunctionTestExecutor::execute(
            &custom_function(None, Some("lookup")),
            serde_json::json!({"city": "beijing"}),
            temp_dir.path(),
        ),
    )
    .await
    .expect("missing plugin validation must finish")
    .expect_err("missing plugin must fail");
    assert_eq!(missing_plugin, "函数未关联插件");

    let missing_export = FunctionTestExecutor::execute(
        &custom_function(Some(1), None),
        serde_json::json!({"city": "beijing"}),
        temp_dir.path(),
    )
    .await
    .expect_err("missing export must fail");
    assert_eq!(missing_export, "函数未指定插件导出函数名");

    let missing_wasm = FunctionTestExecutor::execute(
        &custom_function(Some(1), Some("lookup")),
        serde_json::json!({"city": "beijing"}),
        temp_dir.path(),
    )
    .await
    .expect_err("missing WASM must fail");
    assert!(missing_wasm.contains("WASM 文件不存在"));
}

fn assert_placeholder_not_executable(error: &str) {
    assert!(
        error == "占位函数没有可执行实现，仅用于 LLM 提示词调试"
            || error.contains("function_not_executable")
    );
}

#[tokio::test]
async fn placeholder_function_is_explicitly_non_executable() {
    let temp_dir = tempfile::tempdir().expect("create temp directory");
    let mut function = custom_function(Some(1), Some("echo"));
    function.kind = 3;
    function.required_capabilities = Some(r#"[bad json"#.into());

    let error = FunctionTestExecutor::execute(
        &function,
        serde_json::json!({"query": "prompt-only"}),
        temp_dir.path(),
    )
    .await
    .expect_err("placeholder functions must not be executable");
    assert_placeholder_not_executable(&error);
}

#[tokio::test]
async fn placeholder_function_is_non_executable_from_capability_entrypoint() {
    let temp_dir = tempfile::tempdir().expect("create temp directory");
    let mut function = custom_function(Some(1), Some("echo"));
    function.kind = 3;
    function.required_capabilities = Some(r#"["log.emit"]"#.into());

    let error = FunctionTestExecutor::execute_with_capabilities(
        &function,
        serde_json::json!({"query": "prompt-only"}),
        temp_dir.path(),
        vec!["log.emit".to_string(), "network.http".to_string()],
    )
    .await
    .expect_err("placeholder functions must not be executable");
    assert_placeholder_not_executable(&error);
}

#[tokio::test]
async fn plugin_execution_has_a_hard_timeout() {
    let temp_dir = tempfile::tempdir().expect("create temp directory");
    let wasm_path = temp_dir.path().join("loop.wasm");
    let wasm = wat::parse_str(
        r#"
        (module
          (func (export "loop_forever") (result i32)
            (loop $forever
              br $forever
            )
            i32.const 0
          )
        )
        "#,
    )
    .expect("compile infinite-loop WASM");
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

#[test]
fn function_view_routes_execution_errors_to_a_terminal_state() {
    let source = include_str!("../src/ui/function_view.rs");
    let run_test = source
        .split("fn run_test")
        .nth(1)
        .and_then(|tail| tail.split("impl Render").next())
        .expect("locate run_test implementation");

    assert!(run_test.contains("FunctionTestExecutor::execute"));
    assert!(run_test.contains("TestState::Error"));
    assert!(
        !run_test.contains("return Err("),
        "validation errors must not bypass the terminal state update"
    );
}
