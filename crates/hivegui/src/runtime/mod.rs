//! Function execution runtime for hivegui.
//!
//! Provides direct execution of builtin functions and extism plugins without
//! requiring HTTP requests to remote backend.
//!
//! 变更公开 API 清单（feature 011）：本文件公开类型与安全不变量说明见
//! `specs/011-hivegui-standalone-mode/checklists/changed-public-api.md`。
//!
//! 运行时安全约束：
//! - 仅通过本地 adapter 实现 Function/Tool 执行与取消，不构造 HiveWeb client。
//! - 取消、失败、不可执行（`function_not_executable`）等边界统一走稳定 `FailureCategory`。
//! - 不在该层直接新增数据库/网络副作用。

pub mod builtin_executor;
pub mod capability_adapter;
pub mod desktop_host;
pub mod diagnostics;
pub mod execution;
pub mod function_test_executor;
pub mod plugin_executor;
pub mod provider_resolver;
pub mod tool_adapter;
pub mod workflow_executor;

pub use builtin_executor::BuiltinExecutor;
pub use capability_adapter::{
    AdapterErrorCode, AuditSink, CapabilityAdapter, CapabilityDispatchError, CapabilityHandler,
    DispatchAuditEvent, DispatchOutcome, HandlerRegistry, NullAuditSink,
};
pub use desktop_host::DesktopHostDispatcher;
pub use execution::{
    CANCEL_DEADLINE, CancelHandle, ExecutionId, FailureCategory, FoundationRuntimeComposition,
    LocalExecutionAdapter, LocalExecutionError, LocalExecutionFuture, LocalExecutionOutcome,
    LocalExecutionRequest, LocalFunctionExecutionAdapter,
};
pub use function_test_executor::{
    FUNCTION_TEST_TIMEOUT, FunctionTestExecutor, capability_matches_filter, format_test_output,
    resolve_test_capabilities,
};
pub use plugin_executor::PluginExecutor;
pub use provider_resolver::{
    ProviderAttempt, ProviderCallOutcome, ProviderCallRequest, ProviderError, ProviderErrorKind,
    ProviderResolver, ProviderTransport, TransportError, TransportOutcome, TransportRequest,
};
pub use workflow_executor::{
    WorkflowExecutionError, WorkflowExecutionOutcome, WorkflowExecutor, WorkflowNodeExecutor,
    WorkflowNodeResult, WorkflowValidationError,
};
