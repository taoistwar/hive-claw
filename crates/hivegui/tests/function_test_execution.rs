use std::time::{Duration, Instant};

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

#[tokio::test]
async fn placeholder_function_is_explicitly_non_executable() {
    let temp_dir = tempfile::tempdir().expect("create temp directory");
    let mut function = custom_function(None, None);
    function.kind = 3;

    let error = FunctionTestExecutor::execute(
        &function,
        serde_json::json!({"query": "prompt-only"}),
        temp_dir.path(),
    )
    .await
    .expect_err("placeholder functions must not be executable");

    assert_eq!(error, "占位函数没有可执行实现，仅用于 LLM 提示词调试");
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
