//! T048 [P] [US4] LLM provider resolver contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T048
//! ("在 `crates/hivegui/tests/llm_provider.rs` 编写环境变量优先、设备密钥
//! token、429/5xx/网络/超时 fallback、认证/参数/取消不 fallback 的契约测试").
//!
//! Red public boundaries (T052 will add these):
//!   - `hivegui::runtime::provider_resolver::ProviderResolver`
//!   - `hivegui::runtime::provider_resolver::ProviderTransport`
//!   - `hivegui::runtime::provider_resolver::ProviderCallRequest`
//!   - `hivegui::runtime::provider_resolver::ProviderCallOutcome`
//!   - `hivegui::runtime::provider_resolver::ProviderError`
//!   - `hivegui::runtime::provider_resolver::ProviderErrorKind`
//!   - `hivegui::runtime::provider_resolver::ProviderAttempt`
//!
//! T051 reviewer signs §T048.11 (alongside the existing T050 self-attest
//! for the store half); T052 implementation then makes these tests
//! Green; T054 reruns to record the Green evidence.
//!
//! The tests use a [`ProviderTransport`] trait injection so the
//! resolver's fallback / no-fallback logic can be exercised
//! deterministically without binding to a real HTTP endpoint.

mod support;

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use hive_runtime_core::execution::{
    EventSink, ExecutionContext, PermissionSnapshot, RuntimeEvent, RuntimeEventKind,
};
use hivegui::datasource::llm_provider_store::{
    LlmProviderInput, LlmProviderStore, LlmProviderTokenInput,
};
use hivegui::datasource::{Crypto, llm_store::LlmStore};
use hivegui::runtime::provider_resolver::{
    ProviderAttempt, ProviderCallOutcome, ProviderErrorKind, ProviderResolver, ProviderTransport,
    TransportError, TransportOutcome, TransportRequest,
};
use providers::ChatRequest;
use serde_json::{Value, json};
use support::{CapturedHttpServer, MockHttpResponse, TestWorkspace};
use tokio::{io::AsyncReadExt, net::TcpListener, sync::Notify, task::JoinHandle};

fn device_key(workspace: &TestWorkspace) -> [u8; 32] {
    let bytes = std::fs::read(workspace.device_key_path()).expect("device key bytes");
    bytes[..32].try_into().expect("device key is 32 bytes")
}

#[derive(Debug, Clone)]
struct ScriptedTransport {
    responses: std::sync::Arc<std::sync::Mutex<Vec<TransportOutcome>>>,
}

impl ScriptedTransport {
    fn new(responses: Vec<TransportOutcome>) -> Self {
        Self {
            responses: std::sync::Arc::new(std::sync::Mutex::new(responses)),
        }
    }
}

impl ProviderTransport for ScriptedTransport {
    fn call(&self, _request: TransportRequest) -> Result<TransportOutcome, TransportError> {
        let mut queue = self.responses.lock().expect("scripted mutex");
        Ok(queue.remove(0))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn env_var_token_takes_priority_over_stored_ciphertext() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = LlmProviderStore::new(pool, device_key(&workspace))
        .await
        .expect("store");

    // Set the env var and create a provider that references it.
    // The resolver must use the env-var value at request time,
    // never the stored ciphertext. The provider carries no
    // stored Literal, so the only path to a successful call
    // is via the env-var.
    let env_name = "HIVEGUI_TEST_T048_ENV_TOKEN";
    // SAFETY: only the test process touches this env var.
    unsafe { std::env::set_var(env_name, "env-secret") };
    store
        .create(
            LlmProviderInput::new(
                "openai",
                "openai",
                LlmProviderTokenInput::Env(env_name.to_string()),
                "http://primary.invalid/v1",
            )
            .unwrap(),
        )
        .await
        .expect("create");

    let transport = ScriptedTransport::new(vec![TransportOutcome::Success {
        content: "hi".into(),
    }]);
    let resolver = ProviderResolver::new(store, std::sync::Arc::new(transport)).expect("resolver");
    let request = hivegui::runtime::provider_resolver::ProviderCallRequest::single_message(
        "gpt-4o-mini",
        "hello",
    );

    let outcome = resolver.call(&request).await.expect("call completes");
    let expected = ProviderCallOutcome::Success {
        provider_name: "openai".into(),
        content: "hi".into(),
    };
    assert_eq!(outcome, expected);
}

#[tokio::test(flavor = "current_thread")]
async fn transient_5xx_triggers_fallback_to_next_provider() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = LlmProviderStore::new(pool, device_key(&workspace))
        .await
        .expect("store");

    store
        .create(
            LlmProviderInput::new(
                "primary",
                "openai",
                LlmProviderTokenInput::Literal("sk-primary".into()),
                "http://primary.invalid/v1",
            )
            .unwrap(),
        )
        .await
        .expect("primary");
    store
        .create(
            LlmProviderInput::new(
                "fallback",
                "openai",
                LlmProviderTokenInput::Literal("sk-fallback".into()),
                "http://fallback.invalid/v1",
            )
            .unwrap(),
        )
        .await
        .expect("fallback");

    let transport = ScriptedTransport::new(vec![
        TransportOutcome::TransientFailure,
        TransportOutcome::TransientFailure,
    ]);
    let resolver = ProviderResolver::new(store, std::sync::Arc::new(transport)).expect("resolver");
    let request = hivegui::runtime::provider_resolver::ProviderCallRequest::single_message(
        "gpt-4o-mini",
        "hello",
    );

    let err = resolver
        .call(&request)
        .await
        .expect_err("primary 503 + fallback 503 must surface Exhausted");
    match err.kind() {
        ProviderErrorKind::Exhausted { attempts } => {
            assert_eq!(attempts.len(), 2, "exactly two providers attempted");
            assert_eq!(attempts[0].provider_name(), "primary");
            assert_eq!(attempts[1].provider_name(), "fallback");
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn auth_error_does_not_fall_back() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = LlmProviderStore::new(pool, device_key(&workspace))
        .await
        .expect("store");

    store
        .create(
            LlmProviderInput::new(
                "primary",
                "openai",
                LlmProviderTokenInput::Literal("sk-primary".into()),
                "http://primary.invalid/v1",
            )
            .unwrap(),
        )
        .await
        .expect("primary");
    store
        .create(
            LlmProviderInput::new(
                "fallback",
                "openai",
                LlmProviderTokenInput::Literal("sk-fallback".into()),
                "http://fallback.invalid/v1",
            )
            .unwrap(),
        )
        .await
        .expect("fallback");

    let transport = ScriptedTransport::new(vec![TransportOutcome::AuthFailure]);
    let resolver = ProviderResolver::new(store, std::sync::Arc::new(transport)).expect("resolver");
    let request = hivegui::runtime::provider_resolver::ProviderCallRequest::single_message(
        "gpt-4o-mini",
        "hello",
    );

    let err = resolver.call(&request).await.expect_err("auth surfaces");
    assert!(
        matches!(err.kind(), ProviderErrorKind::Auth { provider_name } if provider_name == "primary"),
        "expected Auth(primary), got {:?}",
        err.kind()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_does_not_fall_back() {
    let workspace = TestWorkspace::new().expect("test workspace");
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = LlmProviderStore::new(pool, device_key(&workspace))
        .await
        .expect("store");

    store
        .create(
            LlmProviderInput::new(
                "primary",
                "openai",
                LlmProviderTokenInput::Literal("sk-primary".into()),
                "http://primary.invalid/v1",
            )
            .unwrap(),
        )
        .await
        .expect("primary");
    store
        .create(
            LlmProviderInput::new(
                "fallback",
                "openai",
                LlmProviderTokenInput::Literal("sk-fallback".into()),
                "http://fallback.invalid/v1",
            )
            .unwrap(),
        )
        .await
        .expect("fallback");

    let transport = ScriptedTransport::new(vec![TransportOutcome::Cancelled]);
    let resolver = ProviderResolver::new(store, std::sync::Arc::new(transport)).expect("resolver");
    let request = hivegui::runtime::provider_resolver::ProviderCallRequest::single_message(
        "gpt-4o-mini",
        "hello",
    )
    .with_cancel_now();

    let err = resolver
        .call(&request)
        .await
        .expect_err("cancel surfaces without fallback");
    assert!(
        matches!(err.kind(), ProviderErrorKind::Cancelled { provider_name } if provider_name == "primary"),
        "expected Cancelled(primary), got {:?}",
        err.kind()
    );
}

#[test]
fn attempt_struct_carries_provider_name() {
    let attempt = ProviderAttempt::new("primary");
    assert_eq!(attempt.provider_name(), "primary");
}

#[test]
fn t052_red_provider_resolver_has_one_workspace_provider_path() {
    let source = include_str!("../src/runtime/provider_resolver.rs");

    for required in [
        "providers::ProviderBuildConfig",
        "providers::build_provider",
        "providers::FallbackProvider",
    ] {
        assert!(
            source.contains(required),
            "T052 must reuse the workspace provider path; missing {required}"
        );
    }

    for forbidden in [
        "reqwest::blocking",
        "pub struct ReqwestProviderTransport",
        "fn build_chat_completions_url",
    ] {
        assert!(
            !source.contains(forbidden),
            "T052 must not retain a second HiveGUI vendor HTTP path: {forbidden}"
        );
    }
}

#[derive(Default)]
struct RecordingEventSink {
    events: Mutex<Vec<RuntimeEvent>>,
}

impl RecordingEventSink {
    fn snapshot(&self) -> Vec<RuntimeEvent> {
        self.events.lock().expect("event mutex").clone()
    }
}

impl EventSink for RecordingEventSink {
    fn emit(&self, event: RuntimeEvent) {
        self.events.lock().expect("event mutex").push(event);
    }
}

fn execution_context() -> (ExecutionContext, Arc<RecordingEventSink>) {
    let sink = Arc::new(RecordingEventSink::default());
    let context = ExecutionContext::new(
        "t052-execution",
        "t052-session",
        "t052-agent",
        PermissionSnapshot::new(BTreeSet::new()),
        sink.clone(),
    )
    .expect("execution context");
    (context, sink)
}

async fn llm_store(workspace: &TestWorkspace) -> (sqlx::SqlitePool, Crypto, LlmStore) {
    workspace
        .ensure_fixture_device_key()
        .expect("device key fixture");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let crypto = Crypto::new(&device_key(workspace));
    let store = LlmStore::new(pool.clone(), crypto.clone());
    store.migrate().await.expect("LLM schema");
    (pool, crypto, store)
}

fn one_message() -> ChatRequest {
    ChatRequest {
        messages: vec![json!({"role": "user", "content": "t052 prompt canary"})],
        ..Default::default()
    }
}

fn openai_success(content: &str) -> MockHttpResponse {
    MockHttpResponse::json(
        200,
        json!({
            "choices": [{
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        }),
    )
}

fn request_json(request: &[u8]) -> Value {
    let split = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP request has header terminator")
        + 4;
    serde_json::from_slice(&request[split..]).expect("request body is JSON")
}

#[tokio::test(flavor = "current_thread")]
async fn t052_red_preset_priority_builds_workspace_fallback_and_emits_runtime_evidence() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (pool, crypto, store) = llm_store(&workspace).await;
    let primary_server = CapturedHttpServer::spawn(vec![MockHttpResponse::json(
        503,
        json!({"error": {"type": "server_error"}}),
    )])
    .await
    .expect("primary server");
    let fallback_server = CapturedHttpServer::spawn(vec![openai_success("fallback-ok")])
        .await
        .expect("fallback server");

    // Deliberately insert the fallback Provider first. The only correct order
    // is Preset -> Model(priority,id) -> Provider, never Provider row id.
    let fallback_provider = store
        .create_provider(
            "fallback-provider",
            "openai",
            &fallback_server.base_url(),
            "sk-fallback",
            "",
        )
        .await
        .expect("fallback provider");
    let primary_provider = store
        .create_provider(
            "primary-provider",
            "openai",
            &primary_server.base_url(),
            "sk-primary",
            "",
        )
        .await
        .expect("primary provider");
    let preset = store
        .create_preset("t052-priority", "", false, 321, 0.25)
        .await
        .expect("preset");
    store
        .create_model("primary-model", preset.id, primary_provider.id, 1)
        .await
        .expect("primary model");
    store
        .create_model("fallback-model", preset.id, fallback_provider.id, 2)
        .await
        .expect("fallback model");

    let resolver = ProviderResolver::from_local_config(pool, crypto).expect("resolver");
    let request = hivegui::runtime::provider_resolver::ProviderCallRequest::for_preset(
        "t052-priority",
        one_message(),
    )
    .with_timeouts(Duration::from_secs(2), Duration::from_secs(4));
    let (context, sink) = execution_context();

    let outcome = resolver
        .call_streaming(&request, &context)
        .await
        .expect("transient primary failure must use the configured fallback");
    assert_eq!(
        outcome,
        ProviderCallOutcome::Success {
            provider_name: "fallback-provider".into(),
            content: "fallback-ok".into(),
        }
    );

    let primary_requests = primary_server.requests();
    let fallback_requests = fallback_server.requests();
    assert_eq!(primary_requests.len(), 1, "primary priority 1 runs first");
    assert_eq!(
        fallback_requests.len(),
        1,
        "fallback priority 2 runs second"
    );
    let primary_body = request_json(&primary_requests[0]);
    let fallback_body = request_json(&fallback_requests[0]);
    assert_eq!(primary_body["model"], "primary-model");
    assert_eq!(fallback_body["model"], "fallback-model");
    assert_eq!(primary_body["max_tokens"], 321);
    assert_eq!(fallback_body["max_tokens"], 321);
    assert_eq!(primary_body["temperature"], 0.25);
    assert_eq!(fallback_body["temperature"], 0.25);

    let events = sink.snapshot();
    let fallback_position = events
        .iter()
        .position(|event| {
            matches!(
                event.kind(),
                RuntimeEventKind::FallbackUsed {
                    from_model,
                    to_model,
                    ..
                } if from_model == "primary-model" && to_model == "fallback-model"
            )
        })
        .expect("fallback_used event");
    let token_position = events
        .iter()
        .position(|event| {
            matches!(
                event.kind(),
                RuntimeEventKind::Token { text, agent_id }
                    if text == "fallback-ok" && agent_id == "t052-agent"
            )
        })
        .expect("provider delta maps to runtime token");
    assert!(
        fallback_position < token_position,
        "fallback transition must be emitted before fallback content"
    );
    assert!(
        events.iter().all(|event| !serde_json::to_string(event)
            .expect("event JSON")
            .contains("t052 prompt canary")),
        "runtime evidence must not leak the prompt"
    );
    assert!(
        context.segment_ms("llm_ms").is_some(),
        "LLM wall time must be recorded separately"
    );
}

struct HangingHttpServer {
    address: std::net::SocketAddr,
    request_seen: Arc<Notify>,
    task: JoinHandle<()>,
}

impl HangingHttpServer {
    async fn spawn() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let request_seen = Arc::new(Notify::new());
        let notify = request_seen.clone();
        let task = tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = [0_u8; 4096];
            if stream.read(&mut buffer).await.unwrap_or(0) > 0 {
                notify.notify_one();
            }
            std::future::pending::<()>().await;
        });
        Ok(Self {
            address,
            request_seen,
            task,
        })
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    async fn wait_for_request(&self) {
        self.request_seen.notified().await;
    }
}

impl Drop for HangingHttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn t052_red_midflight_cancel_drops_http_without_fallback_or_late_token() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (pool, crypto, store) = llm_store(&workspace).await;
    let primary_server = HangingHttpServer::spawn().await.expect("hanging server");
    let fallback_server = CapturedHttpServer::spawn(vec![openai_success("must-not-run")])
        .await
        .expect("fallback server");
    let primary_provider = store
        .create_provider(
            "primary-provider",
            "openai",
            &primary_server.base_url(),
            "sk-primary",
            "",
        )
        .await
        .expect("primary provider");
    let fallback_provider = store
        .create_provider(
            "fallback-provider",
            "openai",
            &fallback_server.base_url(),
            "sk-fallback",
            "",
        )
        .await
        .expect("fallback provider");
    let preset = store
        .create_preset("t052-cancel", "", false, 128, 0.0)
        .await
        .expect("preset");
    store
        .create_model("primary-model", preset.id, primary_provider.id, 1)
        .await
        .expect("primary model");
    store
        .create_model("fallback-model", preset.id, fallback_provider.id, 2)
        .await
        .expect("fallback model");

    let resolver = ProviderResolver::from_local_config(pool, crypto).expect("resolver");
    let request = hivegui::runtime::provider_resolver::ProviderCallRequest::for_preset(
        "t052-cancel",
        one_message(),
    )
    .with_timeouts(Duration::from_secs(30), Duration::from_secs(60));
    let (context, sink) = execution_context();
    let call = resolver.call_streaming(&request, &context);
    tokio::pin!(call);

    tokio::select! {
        _ = primary_server.wait_for_request() => {}
        result = &mut call => panic!("provider call returned before cancellation: {result:?}"),
    }
    context.cancel_with_reason("user_stop");
    let error = tokio::time::timeout(Duration::from_millis(250), &mut call)
        .await
        .expect("cancellation must drop the in-flight HTTP future promptly")
        .expect_err("cancelled provider call cannot succeed");
    assert!(
        matches!(error.kind(), ProviderErrorKind::Cancelled { provider_name }
            if provider_name == "primary-provider"),
        "expected typed primary cancellation, got {:?}",
        error.kind()
    );
    assert!(
        fallback_server.requests().is_empty(),
        "cancellation must not trigger fallback"
    );
    tokio::task::yield_now().await;
    assert!(
        sink.snapshot().iter().all(|event| !matches!(
            event.kind(),
            RuntimeEventKind::FallbackUsed { .. } | RuntimeEventKind::Token { .. }
        )),
        "cancelled HTTP must not emit fallback_used or a late token"
    );
}
