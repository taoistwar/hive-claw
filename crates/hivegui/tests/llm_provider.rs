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

use hivegui::datasource::llm_provider_store::{
    LlmProviderInput, LlmProviderStore, LlmProviderTokenInput,
};
use hivegui::runtime::provider_resolver::{
    ProviderAttempt, ProviderCallOutcome, ProviderErrorKind, ProviderResolver, ProviderTransport,
    TransportError, TransportOutcome, TransportRequest,
};
use support::TestWorkspace;

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
