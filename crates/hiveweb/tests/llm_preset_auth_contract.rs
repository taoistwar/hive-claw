use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hiveweb::runtime::llm::{LlmAdapterError, LlmRegistry};
use providers::{ChatRequest, LLMResponse, StreamDeltaCallback};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

fn write_presets(contents: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hiveweb-llm-auth-{}.toml", uuid::Uuid::new_v4()));
    std::fs::write(&path, contents).expect("write temporary LLM preset config");
    path
}

fn load(contents: &str) -> Result<std::sync::Arc<LlmRegistry>, LlmAdapterError> {
    let path = write_presets(contents);
    let result = LlmRegistry::load_from_path(path.to_str().expect("UTF-8 temp path"));
    std::fs::remove_file(path).expect("remove temporary LLM preset config");
    result
}

#[test]
fn provider_requires_a_credential_reference_by_default() {
    let result = load(
        r#"
[[preset]]
name = "default"
description = "default"
default = true

  [[preset.providers]]
  kind = "openai_compat"
  model = "example"
"#,
    );

    assert!(matches!(result, Err(LlmAdapterError::Parse(_))));
}

#[test]
fn explicit_no_auth_requires_an_explicit_valid_base_url() {
    for base_url in [None, Some("not-a-url")] {
        let base_url_line = base_url
            .map(|value| format!("  base_url = \"{value}\"\n"))
            .unwrap_or_default();
        let result = load(&format!(
            r#"
[[preset]]
name = "default"
description = "default"
default = true

  [[preset.providers]]
  kind = "openai_compat"
  model = "example"
  auth = "none"
{base_url_line}"#
        ));

        assert!(
            matches!(result, Err(LlmAdapterError::Parse(_))),
            "no-auth base URL {base_url:?} must fail closed"
        );
    }
}

#[test]
fn explicit_no_auth_with_self_hosted_base_url_is_valid() {
    let registry = load(
        r#"
[[preset]]
name = "local"
description = "local"
default = true

  [[preset.providers]]
  kind = "openai_compat"
  model = "local-model"
  auth = "none"
  base_url = "http://127.0.0.1:11434/v1"
"#,
    )
    .expect("load explicit no-auth preset");

    assert_eq!(registry.default_name.as_deref(), Some("local"));
}

#[test]
fn explicit_no_auth_rejects_an_api_key_reference() {
    let result = load(
        r#"
[[preset]]
name = "local"
description = "local"
default = true

  [[preset.providers]]
  kind = "openai_compat"
  model = "local-model"
  auth = "none"
  base_url = "http://127.0.0.1:11434/v1"
  api_key_env = "HIVEWEB_TEST_MISSING_LLM_CREDENTIAL_51CA4508"
"#,
    );

    assert!(matches!(result, Err(LlmAdapterError::Parse(_))));
}

async fn serve_openai_response_once(
    listener: TcpListener,
    status: &str,
    response_body: &str,
) -> String {
    let (mut stream, _) = listener.accept().await.expect("accept LLM request");
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut buffer).await.expect("read LLM request");
        assert_ne!(read, 0, "LLM request ended before its headers");
        request.extend_from_slice(&buffer[..read]);
        if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length").then(|| {
                value
                    .trim()
                    .parse::<usize>()
                    .expect("numeric content length")
            })
        })
        .unwrap_or(0);
    while request.len() < header_end + content_length {
        let read = stream.read(&mut buffer).await.expect("read LLM body");
        assert_ne!(read, 0, "LLM request ended before its body");
        request.extend_from_slice(&buffer[..read]);
    }

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
        response_body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .expect("write LLM response");

    String::from_utf8(request).expect("UTF-8 LLM request")
}

async fn same_model_fallback_fixture() -> (Arc<LlmRegistry>, [JoinHandle<String>; 3]) {
    let primary_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind primary provider");
    let primary_address = primary_listener.local_addr().expect("primary address");
    let first_fallback_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind first fallback provider");
    let first_fallback_address = first_fallback_listener
        .local_addr()
        .expect("first fallback address");
    let second_fallback_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind second fallback provider");
    let second_fallback_address = second_fallback_listener
        .local_addr()
        .expect("second fallback address");

    let primary_server = tokio::spawn(serve_openai_response_once(
        primary_listener,
        "503 Service Unavailable",
        r#"{"error":{"type":"server_error","message":"temporarily unavailable"}}"#,
    ));
    let first_fallback_server = tokio::spawn(serve_openai_response_once(
        first_fallback_listener,
        "503 Service Unavailable",
        r#"{"error":{"type":"server_error","message":"first fallback unavailable"}}"#,
    ));
    let second_fallback_server = tokio::spawn(serve_openai_response_once(
        second_fallback_listener,
        "200 OK",
        r#"{"choices":[{"message":{"content":"second fallback ok"},"finish_reason":"stop"}],"usage":{}}"#,
    ));

    let registry = load(&format!(
        r#"
[[preset]]
name = "resilient"
description = "primary with fallback"
default = true
max_tokens = 137
temperature = 0.25

  [[preset.providers]]
  kind = "openai_compat"
  model = "primary-model"
  auth = "none"
  base_url = "http://{primary_address}/v1"

  [[preset.providers]]
  kind = "openai_compat"
  model = "shared-fallback-model"
  auth = "none"
  base_url = "http://{first_fallback_address}/v1"

  [[preset.providers]]
  kind = "openai_compat"
  model = "shared-fallback-model"
  auth = "none"
  base_url = "http://{second_fallback_address}/v1"
"#
    ))
    .expect("load fallback preset");

    (
        registry,
        [
            primary_server,
            first_fallback_server,
            second_fallback_server,
        ],
    )
}

fn explicit_request(primary_model: String) -> ChatRequest {
    ChatRequest {
        messages: vec![serde_json::json!({"role": "user", "content": "hello"})],
        model: Some(primary_model),
        max_tokens: 911,
        temperature: 0.55,
        reasoning_effort: Some("none".to_string()),
        ..Default::default()
    }
}

fn assert_request_settings(request: &str, expected_model: &str) {
    assert!(request.starts_with("POST /v1/chat/completions "));
    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .expect("provider request body");
    let json: serde_json::Value = serde_json::from_str(body).expect("provider request JSON");
    assert_eq!(json["model"], expected_model);
    assert_eq!(json["max_tokens"], 911);
    assert_eq!(json["reasoning_effort"], "none");
    let temperature = json["temperature"]
        .as_f64()
        .expect("numeric request temperature");
    assert!(
        (temperature - 0.55).abs() < 0.000_001,
        "fallback must preserve the caller temperature, got {temperature}"
    );
}

async fn observe_provider_requests(servers: [JoinHandle<String>; 3]) -> Vec<String> {
    let mut requests = Vec::new();
    for server in servers {
        requests.push(
            tokio::time::timeout(Duration::from_secs(2), server)
                .await
                .expect("provider request observed")
                .expect("provider server task"),
        );
    }
    requests
}

async fn execute_same_model_fallback(stream: bool) {
    let (registry, servers) = same_model_fallback_fixture().await;
    let (provider, primary_model) = registry.build_chain(None).expect("build provider chain");
    let (cached_provider, cached_model) = registry
        .build_chain(Some("resilient"))
        .expect("reuse chain");
    assert_eq!(cached_model, primary_model);
    assert!(
        Arc::ptr_eq(&provider, &cached_provider),
        "build_chain must clone the provider chain cached at registry load"
    );

    let deltas = Arc::new(Mutex::new(Vec::new()));
    let callback: StreamDeltaCallback = {
        let deltas = Arc::clone(&deltas);
        Arc::new(move |delta| {
            deltas.lock().expect("delta lock").push(delta);
        })
    };
    let request = explicit_request(primary_model);
    let response: LLMResponse = tokio::time::timeout(Duration::from_secs(5), async {
        if stream {
            provider.chat_stream(request, Some(callback), None).await
        } else {
            provider.chat(request).await
        }
    })
    .await
    .expect("provider chain completes");

    assert_eq!(response.content.as_deref(), Some("second fallback ok"));
    assert_eq!(response.finish_reason, "stop");
    if stream {
        assert_eq!(
            deltas.lock().expect("delta lock").as_slice(),
            ["second fallback ok"]
        );
    } else {
        assert!(deltas.lock().expect("delta lock").is_empty());
    }

    let requests = observe_provider_requests(servers).await;
    assert_request_settings(&requests[0], "primary-model");
    assert_request_settings(&requests[1], "shared-fallback-model");
    assert_request_settings(&requests[2], "shared-fallback-model");
}

#[tokio::test]
async fn build_chain_caches_non_stream_providers_and_indexes_same_model_fallbacks() {
    execute_same_model_fallback(false).await;
}

#[tokio::test]
async fn build_chain_caches_stream_providers_and_preserves_explicit_request_settings() {
    execute_same_model_fallback(true).await;
}

#[tokio::test]
async fn explicit_unknown_preset_fails_closed_while_none_uses_only_the_default() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind default provider");
    let address = listener.local_addr().expect("default provider address");
    let registry = load(&format!(
        r#"
[[preset]]
name = "only-default"
description = "only default"
default = true

  [[preset.providers]]
  kind = "openai_compat"
  model = "default-model"
  auth = "none"
  base_url = "http://{address}/v1"
"#
    ))
    .expect("load default preset");

    let (_, model) = registry.build_chain(None).expect("resolve default preset");
    assert_eq!(model, "default-model");
    assert!(matches!(
        registry.build_chain(Some("missing")),
        Err(LlmAdapterError::Unknown(name)) if name == "missing"
    ));
}
