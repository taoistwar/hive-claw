use std::path::PathBuf;

use async_trait::async_trait;
use log::{debug, error, warn};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::base::Tool;
use super::context::ToolContext;
use super::registry::ToolRegistry;

const SKIP_MODULES: &[&str] = &[
    "base", "schema", "registry", "context", "loader", "config",
    "file_state", "sandbox", "mcp", "__init__", "runtime_state",
];

pub type ToolConstructor = fn(&ToolContext) -> Arc<dyn Tool>;

pub struct ToolLoader {
    test_classes: Option<Vec<ToolConstructor>>,
    discovered: RwLock<Option<Vec<ToolConstructor>>>,
    plugins: RwLock<Option<Vec<ToolConstructor>>>,
}

impl ToolLoader {
    pub fn new(test_classes: Option<Vec<ToolConstructor>>) -> Self {
        Self {
            test_classes,
            discovered: RwLock::new(None),
            plugins: RwLock::new(None),
        }
    }

    pub fn with_test_classes(classes: Vec<ToolConstructor>) -> Self {
        Self::new(Some(classes))
    }

    pub async fn discover(&self) -> Vec<ToolConstructor> {
        if let Some(ref tests) = self.test_classes {
            return tests.clone();
        }

        let guard = self.discovered.read().await;
        if let Some(ref cached) = *guard {
            return cached.clone();
        }
        drop(guard);

        let mut guard = self.discovered.write().await;

        let results = self.discover_inner().await;

        *guard = Some(results.clone());
        results
    }

    async fn discover_inner(&self) -> Vec<ToolConstructor> {
        vec![]
    }

    async fn discover_plugins(&self) -> Vec<ToolConstructor> {
        let guard = self.plugins.read().await;
        if let Some(ref cached) = *guard {
            return cached.clone();
        }
        drop(guard);

        let mut guard = self.plugins.write().await;

        let plugins = self.discover_plugins_inner().await;

        *guard = Some(plugins.clone());
        plugins
    }

    async fn discover_plugins_inner(&self) -> Vec<ToolConstructor> {
        vec![]
    }

    pub async fn load(
        &self,
        ctx: &ToolContext,
        registry: &ToolRegistry,
        scope: &str,
    ) -> Vec<String> {
        let mut registered: Vec<String> = Vec::new();
        let mut builtin_names: HashSet<String> = HashSet::new();

        let builtin_tools = self.discover().await;
        let plugin_tools = self.discover_plugins().await;

        let sources: Vec<(Vec<ToolConstructor>, bool)> = vec![
            (builtin_tools, false),
            (plugin_tools, true),
        ];

        for (source, is_plugin_source) in sources {
            for tool_ctor in source {
                let tool = tool_ctor(ctx);

                let scopes = tool.scopes();
                if !scopes.contains(&scope) {
                    continue;
                }

                if !Self::tool_enabled(&tool, ctx) {
                    continue;
                }

                let name = tool.name().to_string();

                if registry.has(&name).await {
                    if is_plugin_source && builtin_names.contains(&name) {
                        warn!(
                            "Plugin {} skipped: conflicts with built-in tool {}",
                            tool.name(),
                            name
                        );
                        continue;
                    }
                    warn!(
                        "Tool name collision: {} from {} overwrites existing",
                        name,
                        tool.name()
                    );
                }

                registry.register(tool).await;
                registered.push(name.clone());

                if !is_plugin_source {
                    builtin_names.insert(name);
                }
            }
        }

        registered.sort();
        registered
    }

    fn tool_enabled(tool: &Arc<dyn Tool>, ctx: &ToolContext) -> bool {
        let ctx_value = Value::Object(serde_json::Map::new());
        tool.enabled(&ctx_value)
    }
}
