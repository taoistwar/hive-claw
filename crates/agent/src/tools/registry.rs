//! `ToolRegistry` — dynamic registration and execution of agent tools.
//! Port of `nanobot.agent.tools.registry`.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::RwLock;

use super::base::{Tool};
use super::context::RequestContext;

const ERROR_HINT: &str = "\n\n[Analyze the error above and try a different approach.]";

/// Result of `prepare_call` — either a validated tool+params pair, or an
/// error message suitable to return to the LLM.
pub struct PrepareCallResult {
    pub tool: Option<Arc<dyn Tool>>,
    pub params: Value,
    pub error: Option<String>,
}

pub struct ToolRegistry {
    inner: Arc<RwLock<Inner>>,
}

struct Inner {
    tools: HashMap<String, Arc<dyn Tool>>,
    cached_definitions: Option<Vec<Value>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner {
                tools: HashMap::new(),
                cached_definitions: None,
            })),
        }
    }

    pub async fn register(&self, tool: Arc<dyn Tool>) {
        let mut inner = self.inner.write().await;
        let name = tool.name().to_string();
        inner.tools.insert(name, tool);
        inner.cached_definitions = None;
    }

    pub async fn unregister(&self, name: &str) {
        let mut inner = self.inner.write().await;
        inner.tools.remove(name);
        inner.cached_definitions = None;
    }

    pub async fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.inner.read().await.tools.get(name).cloned()
    }

    pub async fn has(&self, name: &str) -> bool {
        self.inner.read().await.tools.contains_key(name)
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.tools.is_empty()
    }

    pub async fn len(&self) -> usize {
        self.inner.read().await.tools.len()
    }

    pub async fn tool_names(&self) -> Vec<String> {
        self.inner.read().await.tools.keys().cloned().collect()
    }

    /// Update context on all registered tools.
    /// Port of Python: iterate `self.tools.tool_names` and call
    /// `set_context` on tools implementing `ContextAware`. In Rust,
    /// `set_tool_context` defaults to no-op, and context-aware tools
    /// override it using interior mutability.
    pub async fn set_all_tool_context(&self, ctx: &RequestContext) {
        let tools = self.inner.read().await;
        for tool in tools.tools.values() {
            tool.set_tool_context(ctx);
        }
    }

    /// Tool definitions with stable ordering (builtins then MCP, both
    /// lexicographically). Cached until the next register/unregister call.
    pub async fn get_definitions(&self) -> Vec<Value> {
        {
            let inner = self.inner.read().await;
            if let Some(cached) = inner.cached_definitions.as_ref() {
                return cached.clone();
            }
        }

        let mut inner = self.inner.write().await;
        let mut builtins: Vec<Value> = Vec::new();
        let mut mcp_tools: Vec<Value> = Vec::new();
        for tool in inner.tools.values() {
            let schema = tool.to_schema();
            let name = schema_name(&schema);
            if name.starts_with("mcp_") {
                mcp_tools.push(schema);
            } else {
                builtins.push(schema);
            }
        }
        builtins.sort_by_key(|s| schema_name(s));
        mcp_tools.sort_by_key(|s| schema_name(s));
        builtins.extend(mcp_tools);
        inner.cached_definitions = Some(builtins.clone());
        builtins
    }

    /// Resolve, cast, and validate one tool call.
    pub async fn prepare_call(&self, name: &str, params: Value) -> PrepareCallResult {
        if !params.is_object() && matches!(name, "write_file" | "read_file") {
            let type_name = match &params {
                Value::Null => "null",
                Value::Bool(_) => "bool",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "list",
                Value::Object(_) => "dict",
            };
            return PrepareCallResult {
                tool: None,
                params,
                error: Some(format!(
                    "Error: Tool '{name}' parameters must be a JSON object, got {type_name}. \
                     Use named parameters: tool_name(param1=\"value1\", param2=\"value2\")"
                )),
            };
        }

        let tool_opt = self.inner.read().await.tools.get(name).cloned();
        let Some(tool) = tool_opt else {
            let names = self
                .inner
                .read()
                .await
                .tools
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            return PrepareCallResult {
                tool: None,
                params,
                error: Some(format!(
                    "Error: Tool '{name}' not found. Available: {names}"
                )),
            };
        };

        let cast = tool.cast_params(params.clone());
        let errors = tool.validate_params(&cast);
        if !errors.is_empty() {
            return PrepareCallResult {
                tool: Some(tool),
                params: cast,
                error: Some(format!(
                    "Error: Invalid parameters for tool '{name}': {}",
                    errors.join("; ")
                )),
            };
        }
        PrepareCallResult {
            tool: Some(tool),
            params: cast,
            error: None,
        }
    }

    /// Execute a tool by name; errors are rendered with a retry hint.
    pub async fn execute(&self, name: &str, params: Value) -> Value {
        let prepared = self.prepare_call(name, params).await;
        if let Some(err) = prepared.error {
            return Value::String(format!("{err}{ERROR_HINT}"));
        }
        let Some(tool) = prepared.tool else {
            return Value::String(format!("Error: Tool '{name}' missing{ERROR_HINT}"));
        };
        match tool.execute(prepared.params).await {
            Ok(Value::String(s)) if s.starts_with("Error") => {
                Value::String(format!("{s}{ERROR_HINT}"))
            }
            Ok(v) => v,
            Err(e) => Value::String(format!("Error executing {name}: {e}{ERROR_HINT}")),
        }
    }
}

fn schema_name(schema: &Value) -> String {
    if let Some(func) = schema.get("function") {
        if let Some(n) = func.get("name").and_then(|v| v.as_str()) {
            return n.to_string();
        }
    }
    schema
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use super::super::base::{ToolExecError};

    struct FakeTool {
        name_: &'static str,
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn name(&self) -> &str {
            self.name_
        }
        fn description(&self) -> String {
            "".into()
        }
        fn parameters(&self) -> Value {
            serde_json::json!({
                "type": "object",
                "properties": {"x": {"type": "integer"}},
                "required": ["x"],
            })
        }
        async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
            Ok(params)
        }
    }

    #[tokio::test]
    async fn prepare_call_missing_tool() {
        let reg = ToolRegistry::new();
        let out = reg.prepare_call("nope", serde_json::json!({})).await;
        assert!(out.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn prepare_call_casts_and_validates() {
        let reg = ToolRegistry::new();
        reg.register(Arc::new(FakeTool { name_: "t1" })).await;
        let out = reg
            .prepare_call("t1", serde_json::json!({"x": "42"}))
            .await;
        assert!(out.error.is_none());
        assert_eq!(out.params["x"], 42);
    }

    #[tokio::test]
    async fn definitions_sort_builtins_first() {
        let reg = ToolRegistry::new();
        reg.register(Arc::new(FakeTool { name_: "mcp_z" })).await;
        reg.register(Arc::new(FakeTool { name_: "a" })).await;
        let defs = reg.get_definitions().await;
        assert_eq!(schema_name(&defs[0]), "a");
        assert_eq!(schema_name(&defs[1]), "mcp_z");
    }
}
