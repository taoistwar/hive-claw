//! Function execution runtime for hivegui
//!
//! Provides direct execution of builtin functions and extism plugins
//! without requiring HTTP requests to remote backend.

pub mod builtin_executor;
pub mod capability_adapter;
pub mod desktop_host;
pub mod diagnostics;
pub mod execution;
pub mod function_test_executor;
pub mod plugin_executor;
pub mod provider_resolver;
pub mod tool_adapter;

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
