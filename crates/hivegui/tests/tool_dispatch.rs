//! T102 [P] [US11] Tool dispatch contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T102
//! ("编写 schema 校验到执行器启动、Function/Workflow 分派、Capability
//! 拒绝和稳定错误测试").
//!
//! The adapter under test is
//! `hivegui::runtime::tool_adapter::LocalToolAdapter`. The real
//! Function/Workflow runtimes are swapped for an injected dispatcher so
//! the routing, capability rejection and stable-error mapping can be
//! asserted without invoking any backend.

use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Value, json};

use hivegui::runtime::execution::{FailureCategory, LocalExecutionOutcome};
use hivegui::runtime::tool_adapter::{
    InMemoryToolDispatcher, LocalToolAdapter, LocalToolDispatcher, ToolAdapterError,
    ToolCallRequest, ToolCallResult, ToolKind, outcome_to_result, tool_error_to_failure,
};

/// A dispatcher that always returns `NotFound` for the first call and a
/// fixed terminal result for subsequent calls.
struct FixedDispatcher {
    error: ToolAdapterError,
}

impl LocalToolDispatcher for FixedDispatcher {
    fn dispatch(
        &self,
        _request: ToolCallRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ToolCallResult, ToolAdapterError>> + Send>>
    {
        let error = self.error.clone();
        Box::pin(async move { Err(error) })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_delegates_function_call_to_dispatcher() {
    let dispatcher = Arc::new(InMemoryToolDispatcher::new());
    let adapter = LocalToolAdapter::new(dispatcher.clone());

    let request = ToolCallRequest::new("json_parse", ToolKind::Function, json!({"x": 1}));
    let result = adapter.dispatch(request.clone()).await.expect("dispatch");
    assert!(result.succeeded);
    assert_eq!(result.call_id, request.call_id);

    let calls = dispatcher.calls();
    assert_eq!(calls.len(), 1, "function call must reach the dispatcher");
    assert_eq!(calls[0].call_id, request.call_id);
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_delegates_workflow_call_to_dispatcher() {
    let dispatcher = Arc::new(InMemoryToolDispatcher::new());
    let adapter = LocalToolAdapter::new(dispatcher.clone());

    let request = ToolCallRequest::new("daily-report", ToolKind::Workflow, json!({}));
    let result = adapter.dispatch(request.clone()).await.expect("dispatch");
    assert!(result.succeeded);

    let calls = dispatcher.calls();
    assert_eq!(calls.len(), 1, "workflow call must reach the dispatcher");
}

#[tokio::test(flavor = "current_thread")]
async fn remote_prefix_is_rejected_without_invoking_dispatcher() {
    let dispatcher = Arc::new(InMemoryToolDispatcher::new());
    let adapter = LocalToolAdapter::new(dispatcher.clone());

    let request = ToolCallRequest::new("https://evil.example/x", ToolKind::Function, json!({}));
    let err = adapter
        .dispatch(request)
        .await
        .expect_err("remote must be rejected");
    match err {
        ToolAdapterError::RemoteForbidden(identifier) => {
            assert_eq!(identifier, "https://evil.example/x");
        }
        other => panic!("expected RemoteForbidden, got {other:?}"),
    }

    assert!(
        dispatcher.calls().is_empty(),
        "a rejected remote call must never reach the dispatcher"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn not_found_error_is_stable() {
    let dispatcher = Arc::new(FixedDispatcher {
        error: ToolAdapterError::NotFound("missing".to_string()),
    });
    let adapter = LocalToolAdapter::new(dispatcher);

    let request = ToolCallRequest::new("missing", ToolKind::Local, json!({}));
    let err = adapter.dispatch(request).await.expect_err("not found");
    assert_eq!(err, ToolAdapterError::NotFound("missing".to_string()));
}

#[test]
fn tool_error_to_failure_categories_are_stable() {
    assert_eq!(
        tool_error_to_failure(&ToolAdapterError::RemoteForbidden("x".into())),
        FailureCategory::Auth
    );
    assert_eq!(
        tool_error_to_failure(&ToolAdapterError::NotFound("x".into())),
        FailureCategory::NotFound
    );
    assert_eq!(
        tool_error_to_failure(&ToolAdapterError::NonTerminal("x".into())),
        FailureCategory::Internal
    );
    assert_eq!(
        tool_error_to_failure(&ToolAdapterError::Cancelled("x".into())),
        FailureCategory::Internal
    );
    assert_eq!(
        tool_error_to_failure(&ToolAdapterError::AtCapacity),
        FailureCategory::Internal
    );
    assert_eq!(
        tool_error_to_failure(&ToolAdapterError::Backend("x".into())),
        FailureCategory::Internal
    );
}

#[test]
fn outcome_conversion_is_stable() {
    let completed = outcome_to_result("c1", LocalExecutionOutcome::Completed);
    assert!(completed.succeeded);
    assert_eq!(completed.call_id, "c1");

    let failed = outcome_to_result(
        "c2",
        LocalExecutionOutcome::Failed(FailureCategory::NotFound),
    );
    assert!(!failed.succeeded);

    let cancelled = outcome_to_result("c3", LocalExecutionOutcome::Cancelled);
    assert!(!cancelled.succeeded);
    assert_eq!(cancelled.output, Value::String("cancelled".to_string()));
}
