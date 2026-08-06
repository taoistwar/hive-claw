use async_trait::async_trait;
use log::info;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Instant;

use super::base::{Tool, ToolExecError};
use super::context::{ContextAware, RequestContext};
use super::runtime_state::RuntimeState;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MyToolConfig {
    #[serde(default = "default_enable")]
    pub enable: bool,
    #[serde(default)]
    pub allow_set: bool,
}

fn default_enable() -> bool {
    true
}

impl Default for MyToolConfig {
    fn default() -> Self {
        Self {
            enable: true,
            allow_set: false,
        }
    }
}

fn has_real_attr<T: AsRef<str>>(obj: &dyn RuntimeState, key: T) -> bool {
    let key = key.as_ref();
    let state = obj.serialize_state();
    if let Some(map) = state.as_object() {
        map.contains_key(key)
    } else {
        false
    }
}

pub struct SubagentStatus {
    pub task_id: String,
    pub label: String,
    pub phase: String,
    pub iteration: u32,
    pub started_at: Instant,
    pub tool_events: Vec<HashMap<String, Value>>,
    pub usage: Option<String>,
    pub error: Option<String>,
    pub stop_reason: Option<String>,
    pub task_description: String,
}

pub trait SubagentManager: Send + Sync {
    fn tool_names(&self) -> Vec<String>;
    fn task_statuses(&self) -> HashMap<String, SubagentStatus>;
    fn cancel_by_session(&self, session_key: &str) -> Vec<String>;
}

pub struct MyTool {
    runtime_state: Arc<std::sync::RwLock<Box<dyn RuntimeState>>>,
    modify_allowed: bool,
    channel: String,
    chat_id: String,
}

use std::sync::Arc;

impl MyTool {
    pub fn new(
        runtime_state: Arc<std::sync::RwLock<Box<dyn RuntimeState>>>,
        modify_allowed: bool,
    ) -> Self {
        Self {
            runtime_state,
            modify_allowed,
            channel: String::new(),
            chat_id: String::new(),
        }
    }

    pub fn config_cls() -> impl Fn() -> MyToolConfig {
        || MyToolConfig::default()
    }

    pub fn enabled(config: &config::Config) -> bool {
        config.tools.my.enable
    }

    const BLOCKED: &[&str] = &[
        "bus",
        "provider",
        "_running",
        "tools",
        "_runtime_vars",
        "runner",
        "sessions",
        "consolidator",
        "dream",
        "auto_compact",
        "context",
        "commands",
        "_mcp_servers",
        "_mcp_stacks",
        "_pending_queues",
        "_session_locks",
        "_active_tasks",
        "_background_tasks",
        "restrict_to_workspace",
        "channels_config",
        "_concurrency_gate",
        "_unified_session",
        "_extra_hooks",
    ];

    const READ_ONLY: &[&str] = &[
        "subagents",
        "_current_iteration",
        "exec_config",
        "web_config",
    ];

    const DENIED_ATTRS: &[&str] = &[
        "__class__",
        "__dict__",
        "__bases__",
        "__subclasses__",
        "__mro__",
        "__init__",
        "__new__",
        "__reduce__",
        "__getstate__",
        "__setstate__",
        "__del__",
        "__call__",
        "__getattr__",
        "__setattr__",
        "__delattr__",
        "__code__",
        "__globals__",
        "func_globals",
        "func_code",
        "__wrapped__",
        "__closure__",
    ];

    const SENSITIVE_NAMES: &[&str] = &[
        "api_key",
        "secret",
        "password",
        "token",
        "credential",
        "private_key",
        "access_token",
        "refresh_token",
        "auth",
    ];

    fn is_sensitive_field_name(name: &str) -> bool {
        let lowered = name.to_lowercase();
        if Self::SENSITIVE_NAMES.contains(&lowered.as_str()) {
            return true;
        }
        lowered
            .split('_')
            .any(|part| Self::SENSITIVE_NAMES.contains(&part))
    }

    const MAX_RUNTIME_KEYS: usize = 64;

    fn audit(&self, action: &str, detail: &str) {
        let session = if !self.channel.is_empty() {
            format!("{}:{}", self.channel, self.chat_id)
        } else {
            "unknown".into()
        };
        info!("self.{} | {} | session:{}", action, detail, session);
    }

    fn resolve_path(&self, path: &str) -> (Option<Value>, Option<String>) {
        let parts: Vec<&str> = path.split('.').collect();
        let mut obj = self.runtime_state.read().unwrap().serialize_state();

        for part in &parts {
            if Self::DENIED_ATTRS.contains(part) || part.starts_with("__") {
                return (None, Some(format!("'{}' is not accessible", part)));
            }
            if Self::BLOCKED.contains(part) {
                return (None, Some(format!("'{}' is not accessible", part)));
            }
            if Self::is_sensitive_field_name(part) {
                return (None, Some(format!("'{}' is not accessible", part)));
            }

            obj = match &obj {
                Value::Object(map) => match map.get(*part).cloned() {
                    Some(v) => v,
                    None => {
                        return (None, Some(format!("'{}' is not accessible", part)));
                    }
                },
                _ => {
                    return (None, Some(format!("'{}' is not accessible", part)));
                }
            };
        }

        (Some(obj), None)
    }

    fn validate_key(key: Option<&str>, label: &str) -> Option<String> {
        match key {
            Some(k) if !k.trim().is_empty() => None,
            _ => Some(format!("Error: '{}' cannot be empty or whitespace", label)),
        }
    }

    #[expect(
        dead_code,
        reason = "retained for the pending detailed subagent status view"
    )]
    fn format_status(st: &SubagentStatus, indent: &str) -> String {
        let elapsed = st.started_at.elapsed().as_secs_f64();
        let tool_summary = st
            .tool_events
            .iter()
            .rev()
            .take(5)
            .map(|e| {
                let name = e.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let status = e.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                format!("{}({})", name, status)
            })
            .collect::<Vec<_>>();
        let tool_summary_str = if tool_summary.is_empty() {
            "none".into()
        } else {
            tool_summary.join(", ")
        };

        let mut lines = vec![
            format!(
                "{}phase: {}, iteration: {}, elapsed: {:.1}s",
                indent, st.phase, st.iteration, elapsed
            ),
            format!("{}tools: {}", indent, tool_summary_str),
            format!("{}usage: {}", indent, st.usage.as_deref().unwrap_or("n/a")),
        ];

        if let Some(ref err) = st.error {
            lines.push(format!("{}error: {}", indent, err));
        }
        if let Some(ref reason) = st.stop_reason {
            lines.push(format!("{}stop_reason: {}", indent, reason));
        }

        lines.join("\n")
    }

    fn format_value(val: &Value, key: &str) -> String {
        match val {
            Value::Object(_) => {
                let map = val.as_object().unwrap();
                if map.is_empty() {
                    if key.is_empty() {
                        "{}".into()
                    } else {
                        format!("{}: {{}}", key)
                    }
                } else {
                    let keys: Vec<&String> = map.keys().collect();
                    if keys.len() <= 5 {
                        let repr = val.to_string();
                        if repr.len() <= 200 {
                            if key.is_empty() {
                                repr
                            } else {
                                format!("{}: {}", key, repr)
                            }
                        } else {
                            Self::format_dict_preview(&keys, key)
                        }
                    } else {
                        Self::format_dict_preview(&keys, key)
                    }
                }
            }
            Value::Array(arr) => {
                if arr.len() > 20 {
                    if key.is_empty() {
                        format!("[{} items]", arr.len())
                    } else {
                        format!("{}: [{} items]", key, arr.len())
                    }
                } else {
                    let repr = val.to_string();
                    if key.is_empty() {
                        repr
                    } else {
                        format!("{}: {}", key, repr)
                    }
                }
            }
            Value::String(s) => {
                if key.is_empty() {
                    format!("{:?}", s)
                } else {
                    format!("{}: {:?}", key, s)
                }
            }
            Value::Number(n) => {
                if key.is_empty() {
                    n.to_string()
                } else {
                    format!("{}: {}", key, n)
                }
            }
            Value::Bool(b) => {
                if key.is_empty() {
                    b.to_string()
                } else {
                    format!("{}: {}", key, b)
                }
            }
            Value::Null => {
                if key.is_empty() {
                    "null".into()
                } else {
                    format!("{}: null", key)
                }
            }
        }
    }

    fn format_dict_preview(keys: &[&String], key: &str) -> String {
        let preview: Vec<String> = keys.iter().take(15).map(|k| k.to_string()).collect();
        let suffix = if keys.len() > 15 { ", ..." } else { "" };
        let content = format!("{{{}}}{}", preview.join(", "), suffix);
        if key.is_empty() {
            content
        } else {
            format!("{}: {}", key, content)
        }
    }

    fn inspect_all(&self) -> String {
        let _state = &self.runtime_state;
        let mut parts: Vec<String> = Vec::new();

        let state_value = self.runtime_state.read().unwrap().serialize_state();
        if let Some(obj) = state_value.as_object() {
            for k in &["max_iterations", "context_window_tokens", "model"] {
                if let Some(v) = obj.get(*k) {
                    parts.push(Self::format_value(v, k));
                }
            }
        }

        if let Some(obj) = state_value.as_object() {
            for k in &[
                "workspace",
                "provider_retry_mode",
                "max_tool_result_chars",
                "_current_iteration",
                "web_config",
                "exec_config",
                "subagents",
            ] {
                if let Some(v) = obj.get(*k) {
                    parts.push(Self::format_value(v, k));
                }
            }
        }

        if let Some(obj) = state_value.as_object()
            && let Some(usage) = obj.get("_last_usage")
        {
            parts.push(Self::format_value(usage, "_last_usage"));
        }

        if let Some(obj) = state_value.as_object()
            && let Some(rv) = obj.get("_runtime_vars")
            && !rv.is_null()
            && !rv.as_object().map(|m| m.is_empty()).unwrap_or(true)
        {
            parts.push(Self::format_value(rv, "scratchpad"));
        }

        parts.join("\n")
    }

    fn inspect(&self, key: Option<&str>) -> String {
        match key {
            None => self.inspect_all(),
            Some(k) => {
                let top = k.split('.').next().unwrap_or("");
                if Self::DENIED_ATTRS.contains(&top) || top.starts_with("__") {
                    return format!("Error: '{}' is not accessible", top);
                }

                let (obj, err) = self.resolve_path(k);
                if let Some(e) = err {
                    if k == "scratchpad"
                        && let Some(obj) = self
                            .runtime_state
                            .read()
                            .unwrap()
                            .serialize_state()
                            .as_object()
                            .cloned()
                        && let Some(rv) = obj.get("_runtime_vars")
                    {
                        if !rv.is_null() {
                            return Self::format_value(rv, "scratchpad");
                        }
                        return "scratchpad is empty".into();
                    }
                    if !k.contains('.')
                        && let Some(obj) = self
                            .runtime_state
                            .read()
                            .unwrap()
                            .serialize_state()
                            .as_object()
                            .cloned()
                        && let Some(rv) = obj.get("_runtime_vars").and_then(|v| v.as_object())
                        && let Some(v) = rv.get(k)
                    {
                        return Self::format_value(v, k);
                    }
                    return format!("Error: {}", e);
                }

                if let Some(v) = obj {
                    Self::format_value(&v, k)
                } else {
                    format!("Error: '{}' not found", k)
                }
            }
        }
    }

    fn modify(&self, key: Option<&str>, value: &Value) -> String {
        let key = match key {
            Some(k) => k,
            None => {
                return Self::validate_key(None, "key")
                    .unwrap_or_else(|| "Error: 'key' cannot be empty or whitespace".into());
            }
        };

        if let Some(err) = Self::validate_key(Some(key), "key") {
            return err;
        }

        let top = key.split('.').next().unwrap_or("");

        if Self::BLOCKED.contains(&top)
            || Self::DENIED_ATTRS.contains(&top)
            || top.starts_with("__")
            || Self::is_sensitive_field_name(top)
        {
            self.audit("modify", &format!("BLOCKED {}", key));
            return format!("Error: '{}' is protected and cannot be modified", key);
        }

        if Self::READ_ONLY.contains(&top) {
            self.audit("modify", &format!("READ_ONLY {}", key));
            return format!("Error: '{}' is read-only and cannot be modified", key);
        }

        if key.contains('.') {
            let parts: Vec<&str> = key.rsplitn(2, '.').collect();
            let leaf = parts[0];
            let parent_path = parts[1];

            if Self::DENIED_ATTRS.contains(&leaf) || leaf.starts_with("__") {
                self.audit("modify", &format!("BLOCKED leaf '{}'", leaf));
                return format!("Error: '{}' is not accessible", leaf);
            }
            if Self::is_sensitive_field_name(leaf) {
                self.audit("modify", &format!("BLOCKED sensitive leaf '{}'", leaf));
                return format!("Error: '{}' is not accessible", leaf);
            }

            let (parent, err) = self.resolve_path(parent_path);
            if let Some(e) = err {
                return format!("Error: {}", e);
            }

            if let Some(mut parent) = parent
                && let Some(map) = parent.as_object_mut()
            {
                map.insert(leaf.to_string(), value.clone());
                self.audit("modify", &format!("{} = {:?}", key, value));
                return format!("Set {} = {:?}", key, value);
            }

            self.audit("modify", &format!("{} = {:?}", key, value));
            return format!("Set {} = {:?}", key, value);
        }

        match key {
            "max_iterations" | "context_window_tokens" | "model" => {
                self.modify_restricted(key, value)
            }
            _ => self.modify_free(key, value),
        }
    }

    fn modify_restricted(&self, key: &str, value: &Value) -> String {
        let (expected_type, min_val, max_val, min_len) = match key {
            "max_iterations" => ("integer", Some(1), Some(100), None),
            "context_window_tokens" => ("integer", Some(4096), Some(1_000_000), None),
            "model" => ("string", None, None, Some(1)),
            _ => return format!("Error: '{}' is not a restricted key", key),
        };

        match expected_type {
            "integer" => {
                let num = match value.as_i64() {
                    Some(n) => n,
                    None => {
                        return format!(
                            "Error: '{}' must be integer, got {}",
                            key,
                            Self::value_type_name(value)
                        );
                    }
                };
                if let Some(min) = min_val
                    && num < min
                {
                    return format!("Error: '{}' must be >= {}", key, min);
                }
                if let Some(max) = max_val
                    && num > max
                {
                    return format!("Error: '{}' must be <= {}", key, max);
                }
            }
            "string" => {
                let s = match value.as_str() {
                    Some(s) => s,
                    None => {
                        return format!(
                            "Error: '{}' must be string, got {}",
                            key,
                            Self::value_type_name(value)
                        );
                    }
                };
                if let Some(min_len) = min_len
                    && s.len() < min_len
                {
                    return format!("Error: '{}' must be at least {} characters", key, min_len);
                }
            }
            _ => {}
        }

        let old = self
            .runtime_state
            .read()
            .unwrap()
            .get_field(key)
            .unwrap_or(Value::Null);

        self.runtime_state
            .write()
            .unwrap()
            .set_field(key, value.clone());

        if key == "model" {
            self.runtime_state
                .write()
                .unwrap()
                .set_field("_active_preset", Value::Null);
        }
        if key == "max_iterations" {
            todo!("TODO: sync subagent runtime limits if applicable")
        }

        self.audit("modify", &format!("{}: {:?} -> {:?}", key, old, value));
        format!("Set {} = {:?} (was {:?})", key, value, old)
    }

    fn modify_free(&self, key: &str, value: &Value) -> String {
        if has_real_attr(self.runtime_state.read().unwrap().as_ref(), key) {
            let old = self
                .runtime_state
                .read()
                .unwrap()
                .get_field(key)
                .unwrap_or(Value::Null);

            if old.is_string() || old.is_number() || old.is_boolean() {
                let old_type = Self::value_type_name(&old);
                let new_type = Self::value_type_name(value);
                if old_type != new_type && !(old_type == "float" && new_type == "integer") {
                    self.audit(
                        "modify",
                        &format!(
                            "REJECTED type mismatch {}: expects {}, got {}",
                            key, old_type, new_type
                        ),
                    );
                    return format!("Error: '{}' expects {}, got {}", key, old_type, new_type);
                }
            }

            self.runtime_state
                .write()
                .unwrap()
                .set_field(key, value.clone());
            self.audit("modify", &format!("{}: {:?} -> {:?}", key, old, value));
            return format!("Set {} = {:?} (was {:?})", key, value, old);
        }

        if (value.is_array() || value.is_object())
            && let Some(err) = Self::validate_json_safe(value, 0)
        {
            self.audit("modify", &format!("REJECTED {}: {}", key, err));
            return format!("Error: {}", err);
        }

        if value.is_object() || value.is_array() {
            let rv = self.runtime_state.read().unwrap().get_runtime_vars();
            if !rv.contains_key(key) && rv.len() >= Self::MAX_RUNTIME_KEYS {
                self.audit(
                    "modify",
                    &format!(
                        "REJECTED {}: max keys ({}) reached",
                        key,
                        Self::MAX_RUNTIME_KEYS
                    ),
                );
                return format!(
                    "Error: scratchpad is full (max {} keys). Remove unused keys first.",
                    Self::MAX_RUNTIME_KEYS
                );
            }
        }

        let old = self.runtime_state.read().unwrap().get_runtime_var(key);
        self.runtime_state
            .write()
            .unwrap()
            .set_runtime_var(key, value.clone());
        self.audit(
            "modify",
            &format!("scratchpad.{}: {:?} -> {:?}", key, old, value),
        );
        format!("Set scratchpad.{} = {:?}", key, value)
    }

    fn validate_json_safe(value: &Value, depth: usize) -> Option<String> {
        if depth > 10 {
            return Some("value nesting too deep (max 10 levels)".into());
        }
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
            Value::Array(arr) => {
                for (i, item) in arr.iter().enumerate() {
                    if let Some(err) = Self::validate_json_safe(item, depth + 1) {
                        return Some(format!("list[{}] contains {}", i, err));
                    }
                }
                None
            }
            Value::Object(map) => {
                for (k, v) in map {
                    if let Some(err) = Self::validate_json_safe(v, depth + 1) {
                        return Some(format!("dict key '{}' contains {}", k, err));
                    }
                }
                None
            }
        }
    }

    fn value_type_name(val: &Value) -> &'static str {
        match val {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(n) => {
                if n.is_i64() || n.is_u64() {
                    "integer"
                } else {
                    "float"
                }
            }
            Value::String(_) => "string",
            Value::Array(_) => "list",
            Value::Object(_) => "dict",
        }
    }
}

impl ContextAware for MyTool {
    fn set_context(&mut self, ctx: &RequestContext) {
        self.channel = ctx.channel.clone();
        self.chat_id = ctx.chat_id.clone();
    }
}

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str {
        "my"
    }

    fn description(&self) -> String {
        let base = concat!(
            "Check and set your own runtime state.\n",
            "Actions: check, set.\n",
            "- check (no key): full config overview — start here.\n",
            "- check (key): drill into a value. Dot-paths allowed ",
            "(e.g. '_last_usage.prompt_tokens', 'web_config.enable').\n",
            "- set (key, value): change config or store notes in your scratchpad. ",
            "Scratchpad keys persist across turns but not restarts.\n",
            "Key values: _current_iteration (current progress), ",
            "max_iterations - _current_iteration = remaining iterations.\n",
            "Note: web_config and exec_config are readable but read-only.\n",
            "\n",
            "When to use:\n",
            "- User asks about your model, settings, or token usage → check that key.\n",
            "- A tool fails or behaves unexpectedly → check the related config to diagnose.\n",
            "- User asks you to remember a preference for this session → set to store it in your scratchpad.\n",
            "- About to start a large task → check context_window_tokens and max_iterations first."
        ).to_string();

        if !self.modify_allowed {
            format!("{}\nREAD-ONLY MODE: set is disabled.", base)
        } else {
            format!(
                "{}\nIMPORTANT: Before setting state, predict the potential impact. If the operation could cause crashes or instability (e.g. changing model), warn the user first.",
                base
            )
        }
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["check", "set"],
                    "description": "Action to perform"
                },
                "key": {
                    "type": "string",
                    "description": "Dot-path for check/set. Examples: 'max_iterations', 'workspace', 'provider_retry_mode'. For check without key, shows all config values."
                },
                "value": {
                    "description": "New value (for set). Type must match target (int for max_iterations/context_window_tokens, str for model)."
                }
            },
            "required": ["action"]
        })
    }

    fn config_key(&self) -> &str {
        "my"
    }

    fn plugin_discoverable(&self) -> bool {
        false
    }

    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolExecError::InvalidParams("missing required parameter: action".into())
            })?;

        let key = params.get("key").and_then(|v| v.as_str());
        let value = params.get("value").cloned().unwrap_or(Value::Null);

        match action {
            "inspect" | "check" => Ok(Value::String(self.inspect(key))),
            "modify" | "set" => {
                if !self.modify_allowed {
                    return Ok(Value::String(
                        "Error: set is disabled (tools.my.allow_set is false)".into(),
                    ));
                }
                Ok(Value::String(self.modify(key, &value)))
            }
            _ => Ok(Value::String(format!("Unknown action: {}", action))),
        }
    }
}
