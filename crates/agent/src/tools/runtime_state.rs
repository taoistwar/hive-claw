use std::any::Any;
use std::collections::HashMap;

use serde_json::Value;

pub trait RuntimeState: Any + Send + Sync {
    fn model(&self) -> &str;
    fn max_iterations(&self) -> usize;
    fn current_iteration(&self) -> usize;
    fn tool_names(&self) -> Vec<String>;
    fn workspace(&self) -> &str;
    fn provider_retry_mode(&self) -> &str;
    fn max_tool_result_chars(&self) -> usize;
    fn context_window_tokens(&self) -> Option<usize>;
    fn web_config(&self) -> Option<&Value>;
    fn exec_config(&self) -> Option<&Value>;
    fn subagents(&self) -> Option<&dyn Any>;
    fn runtime_vars(&self) -> Option<&dyn Any>;
    fn last_usage(&self) -> Option<&dyn Any>;
    fn sync_subagent_runtime_limits(&self);
    fn model_preset(&self) -> Option<&str>;
    fn active_preset(&self) -> Option<&str>;
    fn set_active_preset(&mut self, preset: Option<String>);

    fn get_field(&self, key: &str) -> Option<Value>;
    fn set_field(&mut self, key: &str, value: Value);
    fn get_runtime_vars(&self) -> HashMap<String, Value>;
    fn get_runtime_var(&self, key: &str) -> Option<Value>;
    fn set_runtime_var(&mut self, key: &str, value: Value);
    fn serialize_state(&self) -> Value;
}
