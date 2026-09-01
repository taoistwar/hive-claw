//! Builtin function executor
//!
//! Executes pure computation builtin functions directly using the shared crate.

use serde_json::Value;

pub struct BuiltinExecutor;

impl BuiltinExecutor {
    /// Execute a builtin function by identifier
    pub fn execute(identifier: &str, input: Value) -> Result<Value, String> {
        hive_builtins::BuiltinRegistry::execute(identifier, input).map_err(|e| e.to_string())
    }
}
