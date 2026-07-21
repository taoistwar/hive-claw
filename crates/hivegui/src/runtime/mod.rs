//! Function execution runtime for hivegui
//!
//! Provides direct execution of builtin functions and extism plugins
//! without requiring HTTP requests to hiveweb.

pub mod builtin_executor;
pub mod desktop_host;
pub mod function_test_executor;
pub mod plugin_executor;

pub use builtin_executor::BuiltinExecutor;
pub use desktop_host::DesktopHostDispatcher;
pub use function_test_executor::{
    FUNCTION_TEST_TIMEOUT, FunctionTestExecutor, capability_matches_filter, format_test_output,
    resolve_test_capabilities,
};
pub use plugin_executor::PluginExecutor;
