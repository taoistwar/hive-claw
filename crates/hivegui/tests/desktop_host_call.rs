use std::time::Duration;

use hivegui::runtime::{DesktopHostDispatcher, PluginExecutor};

#[tokio::test]
async fn host_call_uses_standard_permission_and_unknown_capability_errors() {
    let dispatcher = DesktopHostDispatcher::default();
    let denied = dispatcher
        .dispatch(
            r#"{"capability":"network.http","args":{"method":"GET","url":"http://127.0.0.1"}}"#,
            &[],
        )
        .await;
    let denied: serde_json::Value = serde_json::from_str(&denied).expect("parse denied reply");
    assert_eq!(denied["ok"], false);
    assert_eq!(denied["code"], 4030);
    assert!(denied.get("data").is_none());

    let wrong_name = dispatcher
        .dispatch(
            r#"{"capability":"network.http","args":{}}"#,
            &["net".to_string()],
        )
        .await;
    let wrong_name: serde_json::Value =
        serde_json::from_str(&wrong_name).expect("parse wrong-name reply");
    let message = wrong_name["message"].as_str().expect("denial message");
    assert!(message.contains("network.http"));
    assert!(message.contains("当前已选 Capability：net"));

    let unknown = dispatcher
        .dispatch(
            r#"{"capability":"unknown.capability","args":{}}"#,
            &["unknown.capability".to_string()],
        )
        .await;
    let unknown: serde_json::Value = serde_json::from_str(&unknown).expect("parse unknown reply");
    assert_eq!(unknown["ok"], false);
    assert_eq!(unknown["code"], 4045);
}

#[tokio::test]
async fn network_http_capability_forwards_the_request_and_response() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let address = listener.local_addr().expect("read test server address");
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept request");
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).await.expect("read request");
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"weather\":\"sun\"}",
            )
            .await
            .expect("write response");
    });

    let envelope = serde_json::json!({
        "capability": "network.http",
        "args": {
            "method": "GET",
            "url": format!("http://{address}/weather")
        }
    })
    .to_string();
    let reply = DesktopHostDispatcher::default()
        .dispatch(&envelope, &["network.http".to_string()])
        .await;
    server.await.expect("test server completes");

    let reply: serde_json::Value = serde_json::from_str(&reply).expect("parse host reply");
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["data"]["status"], 200);
    assert_eq!(reply["data"]["body"], r#"{"weather":"sun"}"#);
}

#[tokio::test]
async fn plugin_importing_host_call_can_be_built_by_the_desktop_executor() {
    let temp_dir = tempfile::tempdir().expect("create temp directory");
    let wasm_path = temp_dir.path().join("imports-host-call.wasm");
    let wasm = wat::parse_str(
        r#"
        (module
          (type $host-call-type (func (param i64) (result i64)))
          (import "extism:host/user" "host_call" (func $host_call (type $host-call-type)))
          (func (export "noop") (result i32)
            i32.const 0
          )
        )
        "#,
    )
    .expect("compile host_call import fixture");
    std::fs::write(&wasm_path, wasm).expect("write WASM fixture");

    let result = PluginExecutor::execute_with_capabilities(
        &wasm_path,
        "noop",
        "{}",
        Duration::from_secs(1),
        Vec::new(),
    )
    .await;

    if let Err(error) = result {
        assert!(
            !error.contains("host_call has not been defined") && !error.contains("unknown import"),
            "host_call ABI was not registered: {error}"
        );
    }
}

#[test]
fn function_test_passes_required_capabilities_to_the_plugin_executor() {
    let source = include_str!("../src/runtime/function_test_executor.rs");
    assert!(source.contains("required_capabilities"));
    assert!(source.contains("execute_with_capabilities"));
}

#[test]
fn function_test_reads_current_text_input_values_before_execution() {
    let source = include_str!("../src/ui/function_view.rs");
    let run_test = source
        .split("fn run_test")
        .nth(1)
        .and_then(|tail| tail.split("impl Render").next())
        .expect("locate run_test implementation");

    assert!(run_test.contains("test_input_states"));
    assert!(run_test.contains("state.read(cx).value()"));
}

#[test]
fn function_test_dialog_refreshes_capabilities_and_uses_a_compact_multi_select() {
    let source = include_str!("../src/ui/function_view.rs");

    assert!(source.contains("test_capabilities: HashSet<String>"));
    assert!(source.contains("test_capability_select_open: bool"));
    assert!(source.contains("test_capability_scroll: ScrollHandle"));
    assert!(source.contains("test_capability_filter: String"));
    assert!(source.contains("test_capability_filter_input"));
    assert!(source.contains("允许的 Capabilities"));
    assert!(source.contains("test-capability-selector"));
    assert!(source.contains("过滤 Capability..."));
    assert!(source.contains("selector_menu"));
    assert!(source.contains("track_scroll(&self.test_capability_scroll)"));
    assert!(source.contains("capability_matches_filter"));
    assert!(!source.contains("☑"));
    assert!(!source.contains("☐"));

    let show_dialog = source
        .split("fn show_test_dialog")
        .nth(1)
        .and_then(|tail| tail.split("fn hide_test_dialog").next())
        .expect("locate show_test_dialog implementation");
    assert!(show_dialog.contains("test_capabilities"));
    assert!(show_dialog.contains("refresh_test_capabilities"));

    let refresh = source
        .split("fn refresh_test_capabilities")
        .nth(1)
        .and_then(|tail| tail.split("fn show_test_dialog").next())
        .expect("locate capability refresh implementation");
    assert!(refresh.contains("Capability::list"));
    assert!(refresh.contains("required_capabilities"));
    assert!(refresh.contains("resolve_test_capabilities"));

    let run_test = source
        .split("fn run_test")
        .nth(1)
        .and_then(|tail| tail.split("impl Render").next())
        .expect("locate run_test implementation");
    assert!(run_test.contains("test_capabilities"));
    assert!(run_test.contains("FunctionTestExecutor::execute_with_capabilities"));
}
