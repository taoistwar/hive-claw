//! Tool test runner (T127 / US7)
//!
//! 为单个 Tool 创建隔离测试环境：加载目标 Tool + Always Tools + Always Skills → 执行单轮对话 → 返回结果

use providers::{ChatRequest, LLMProvider, LlmCallOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::MySqlPool;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::orchestrator::{OrchestratorDeps, build_tools_schema_simple, handle_workspace_tool};
use crate::runtime::execution_context::RuntimeExecutionContext;
use crate::runtime::llm::{LlmAdapterError, LlmRegistry};
use crate::runtime::llm_audit::{LlmAuditGuard, LlmAuditSource};
use crate::services::agent::{AgentContent, ToolRef};
use agent::context::{AgentContext, ContextConfig, UserInput};

const TOOL_TEST_LOG_DIR: &str = "/tmp/tool_test_logs";
const MAX_TRACE_ID_LEN: usize = 64;

pub fn generate_tool_test_trace_id() -> String {
    format!("tool_test_{}", uuid::Uuid::new_v4().simple())
}

fn is_safe_trace_id(trace_id: &str) -> bool {
    !trace_id.is_empty()
        && trace_id.len() <= MAX_TRACE_ID_LEN
        && trace_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn tool_test_log_path(trace_id: &str) -> Option<PathBuf> {
    if !is_safe_trace_id(trace_id) {
        return None;
    }
    Some(Path::new(TOOL_TEST_LOG_DIR).join(format!("{trace_id}.log")))
}

type ToolTestRow = (
    i64,
    String,
    String,
    String,
    i8,
    Option<i64>,
    Option<i64>,
    Value,
    Option<i64>,
    Option<String>,
    Option<Value>,
);

#[derive(Debug, Deserialize)]
pub struct TestToolRequest {
    pub message: String,
    pub model_preset: Option<String>,
    pub trace_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TestToolResult {
    pub assistant_content: String,
    pub has_tool_calls: bool,
    pub tool_calls: Vec<TestToolCallRecord>,
}

#[derive(Debug, Serialize)]
pub struct TestToolCallRecord {
    pub tool_name: String,
    pub arguments: Value,
    pub result: TestToolCallOutcome,
}

#[derive(Debug, Serialize)]
pub struct TestToolCallOutcome {
    pub success: bool,
    pub content: Value,
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolTestError {
    #[error("model preset unknown: {0}")]
    ModelPresetUnknown(String),
    #[error("{0}")]
    Failed(String),
}

impl From<String> for ToolTestError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

struct ResolvedToolTestLlmTarget {
    provider: Arc<dyn LLMProvider>,
    model: String,
    preset_name: String,
    max_tokens: u32,
    temperature: f32,
}

trait ToolTestLlmRegistry {
    fn resolve_preset(
        &self,
        requested_preset: Option<&str>,
    ) -> Result<(String, u32, f32), LlmAdapterError>;

    fn build_chain(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError>;
}

impl ToolTestLlmRegistry for LlmRegistry {
    fn resolve_preset(
        &self,
        requested_preset: Option<&str>,
    ) -> Result<(String, u32, f32), LlmAdapterError> {
        let entry = self.resolve(requested_preset)?;
        Ok((entry.name.clone(), entry.max_tokens, entry.temperature))
    }

    fn build_chain(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError> {
        LlmRegistry::build_chain(self, preset_name)
    }
}

fn resolve_tool_test_llm_target<R: ToolTestLlmRegistry + ?Sized>(
    registry: &R,
    execution_context: &RuntimeExecutionContext,
    tool_id: i64,
    requested_preset: Option<&str>,
) -> Result<ResolvedToolTestLlmTarget, ToolTestError> {
    let (preset_name, max_tokens, temperature) = match registry.resolve_preset(requested_preset) {
        Ok(preset) => preset,
        Err(LlmAdapterError::Unknown(name)) => {
            let mut audit = LlmAuditGuard::new(
                execution_context.clone(),
                None,
                requested_preset,
                LlmAuditSource::ToolTest { tool_id },
            );
            audit.finish_model_preset_unknown();
            return Err(ToolTestError::ModelPresetUnknown(name));
        }
        Err(error) => {
            return Err(ToolTestError::Failed(format!(
                "LLM preset resolution failed: {error}"
            )));
        }
    };
    let (provider, model) = registry
        .build_chain(Some(&preset_name))
        .map_err(|error| ToolTestError::Failed(format!("LLM provider: {error}")))?;

    Ok(ResolvedToolTestLlmTarget {
        provider,
        model,
        preset_name,
        max_tokens,
        temperature,
    })
}

#[derive(Clone)]
pub struct DebugLogger {
    trace_id: Option<String>,
    lines: Arc<Mutex<Vec<String>>>,
}

impl DebugLogger {
    pub fn new(trace_id: Option<String>) -> Self {
        Self {
            trace_id: trace_id.filter(|trace_id| is_safe_trace_id(trace_id)),
            lines: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn log(&self, msg: &str) {
        let line = format!("[tool_test] {}", msg);
        if !crate::app_mode::get().is_production()
            && let Some(path) = self.trace_id.as_deref().and_then(tool_test_log_path)
        {
            let _ = std::fs::create_dir_all(TOOL_TEST_LOG_DIR);
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| {
                    use std::io::Write;
                    writeln!(f, "{}", line)
                });
        }
        if let Ok(mut lines) = self.lines.lock() {
            lines.push(line);
        }
    }

    pub fn dump(&self) -> String {
        self.lines.lock().map(|l| l.join("\n")).unwrap_or_default()
    }

    pub fn trace_id(&self) -> Option<&str> {
        self.trace_id.as_deref()
    }
}

pub async fn run_tool_test(
    pool: &MySqlPool,
    deps: &OrchestratorDeps,
    tool_id: i64,
    req: TestToolRequest,
) -> Result<TestToolResult, ToolTestError> {
    let logger = DebugLogger::new(req.trace_id.clone());
    logger.log(&format!(
        "START tool_id={} message_bytes={}",
        tool_id,
        req.message.len()
    ));
    // 1. 查询目标 tool
    let tool_row: Option<ToolTestRow> = sqlx::query_as(
        r#"SELECT t.id, t.identifier, t.name, t.description, t.kind,
                      t.function_id, t.workflow_id, t.input_schema,
                      f.plugin_id, f.plugin_export,
                      t.required_capabilities
               FROM tools t
               LEFT JOIN functions f ON f.id = t.function_id
               WHERE t.id = ?"#,
    )
    .bind(tool_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("tool lookup: {e}"))?;

    let Some((
        tid,
        t_ident,
        t_name,
        t_desc,
        t_kind,
        t_fid,
        t_wid,
        t_input_schema,
        t_plugin_id,
        t_plugin_export,
        t_caps_raw,
    )) = tool_row
    else {
        logger.log(&format!("FAIL: tool id={} not found", tool_id));
        return Err(ToolTestError::Failed(format!(
            "tool id={tool_id} not found"
        )));
    };
    logger.log(&format!("STEP1 OK: tool found id={} kind={}", tid, t_kind));

    let t_caps: Vec<String> = t_caps_raw
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    // 2. 加载 always tools（排除目标 tool 避免重复）
    let always_tools: Vec<ToolTestRow> = sqlx::query_as(
        r#"SELECT t.id, t.identifier, t.name, t.description, t.kind,
                      t.function_id, t.workflow_id, t.input_schema,
                      f.plugin_id, f.plugin_export,
                      t.required_capabilities
               FROM tools t
               LEFT JOIN functions f ON f.id = t.function_id
               WHERE t.is_always = 1 AND t.id != ?"#,
    )
    .bind(tool_id)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("always tools: {e}"))?;
    logger.log(&format!(
        "STEP2 OK: loaded {} always_tools",
        always_tools.len()
    ));

    // 3. 构建 ToolRef 列表
    let mut tools: Vec<ToolRef> = Vec::new();
    // 目标 tool
    tools.push(ToolRef {
        id: tid,
        identifier: t_ident.clone(),
        name: t_name.clone(),
        description: t_desc.clone(),
        kind: t_kind,
        function_id: t_fid,
        function_identifier: None,
        workflow_id: t_wid,
        input_schema: t_input_schema,
        plugin_id: t_plugin_id,
        plugin_export: t_plugin_export,
        is_builtin_function: t_kind == 1 && t_plugin_id.is_none(),
        is_meta_tool: t_kind == 1 && t_fid.is_none(),
        required_capabilities: t_caps,
    });
    // always tools
    for (id, ident, name, desc, kind, fid, wid, input_schema, pid, pexport, caps_raw) in
        always_tools
    {
        let is_builtin = kind == 1 && pid.is_none();
        let is_meta = kind == 1 && fid.is_none();
        let required_capabilities: Vec<String> = caps_raw
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        tools.push(ToolRef {
            id,
            identifier: ident,
            name,
            description: desc,
            kind,
            function_id: fid,
            function_identifier: None,
            workflow_id: wid,
            input_schema,
            plugin_id: pid,
            plugin_export: pexport,
            is_builtin_function: is_builtin,
            is_meta_tool: is_meta,
            required_capabilities,
        });
    }

    // 4. 加载 always skills 拼入 system prompt
    let skills: Vec<(String,)> = sqlx::query_as("SELECT content FROM skills WHERE is_always = 1")
        .fetch_all(pool)
        .await
        .map_err(|e| format!("always skills: {e}"))?;
    logger.log(&format!("STEP3 OK: loaded {} skills", skills.len()));

    let mut system_prompt = format!(
        "You are a tool execution test harness.\n\
         The target tool under test is `{t_ident}` (display name: \"{t_name}\").\n\
         Goal: You MUST invoke the target tool `{t_ident}` at least once so its real execution result can be verified.\n\
         Steps:\n\
         1. Read the target tool's `description` and `parameters` schema.\n\
         2. Construct valid arguments (use sensible defaults if the schema has no required fields).\n\
         3. Call the tool by issuing a `tool_calls` entry — do NOT just describe what the tool would do.\n\
         4. After receiving the tool result, summarize the outcome briefly for the user.\n\
         You may also use any `is_always` tools if they are clearly needed, but the primary objective is to execute `{t_ident}` itself.",
        t_ident = t_ident,
        t_name = t_name,
    );
    for (md,) in &skills {
        system_prompt.push_str("\n\n--- SKILL ---\n\n");
        system_prompt.push_str(md);
    }

    // 测试模式：授予所有 capability 权限，以便插件能自由调用 network.http 等能力
    let permissions: Vec<String> = crate::runtime::capability::CAPABILITIES
        .iter()
        .map(|c| c.name.to_string())
        .collect();

    let ctx = AgentContent {
        agent_id: 0, // 测试模式，不需要真实 agent_id
        identifier: format!("test_tool_{}", tool_id),
        system_prompt,
        model_preset: req.model_preset,
        tools,
        permissions,
        children: vec![],
        hooks: std::collections::HashMap::new(),
    };
    logger.log(&format!(
        "STEP4 OK: AgentContext built, tools_count={}",
        ctx.tools.len()
    ));

    // 5. 构建 LLM request
    let requested_preset = ctx.model_preset.as_deref();
    let ResolvedToolTestLlmTarget {
        provider,
        model,
        preset_name: resolved_preset,
        max_tokens,
        temperature,
    } = resolve_tool_test_llm_target(
        deps.llm.as_ref(),
        &deps.execution_context,
        tool_id,
        requested_preset,
    )?;
    logger.log("STEP5 OK: LLM provider built");

    let tools_schema = build_tools_schema_simple(&ctx.tools);

    let messages: Vec<Value> = vec![
        json!({"role": "system", "content": ctx.system_prompt}),
        json!({"role": "user", "content": req.message}),
    ];

    let chat_req = ChatRequest {
        model: Some(model),
        messages,
        max_tokens,
        temperature,
        tools: if tools_schema.is_empty() {
            None
        } else {
            Some(tools_schema)
        },
        tool_choice: None,
        reasoning_effort: None,
    };
    logger.log(&format!(
        "STEP6 OK: chat_req built, has_tools={}",
        chat_req.tools.is_some()
    ));

    let mut llm_audit = LlmAuditGuard::new(
        deps.execution_context.clone(),
        None,
        Some(&resolved_preset),
        LlmAuditSource::ToolTest { tool_id },
    );
    let options = LlmCallOptions::default().with_fallback_callback(llm_audit.on_fallback());
    logger.log("STEP7: calling chat_stream_with_options...");
    let resp = provider
        .chat_stream_with_options(chat_req, None, None, options)
        .await;
    llm_audit.finish_response(&resp);
    logger.log(&format!(
        "STEP7 DONE: LLM responded, is_error={}, content_len={}",
        resp.is_error(),
        resp.content.as_ref().map(|s| s.len()).unwrap_or(0)
    ));

    if resp.is_error() {
        let err_msg = resp.content.clone().unwrap_or_else(|| "unknown".into());
        logger.log("FAIL: LLM invocation failed");
        return Err(ToolTestError::Failed(format!("LLM error: {err_msg}")));
    }
    logger.log(&format!(
        "STEP7 OK: LLM success, tool_calls_count={}",
        resp.tool_calls.len()
    ));

    let assistant_content = resp.content.unwrap_or_default();
    let tool_calls = resp.tool_calls;

    if tool_calls.is_empty() {
        logger.log("STEP8 OK: no tool_calls, returning direct response");
        return Ok(TestToolResult {
            assistant_content,
            has_tool_calls: false,
            tool_calls: vec![],
        });
    }
    logger.log(&format!("STEP8: executing {} tool_calls", tool_calls.len()));

    // 6. 执行工具调用
    let mut records = Vec::new();
    for (idx, tc) in tool_calls.iter().enumerate() {
        let tool_name = tc.name.clone();
        let args = tc.arguments.clone();
        logger.log(&format!(
            "STEP9.{}: executing tool with arg_count={}",
            idx,
            args.len()
        ));

        let tool_ref = ctx.tools.iter().find(|t| t.identifier == tc.name);
        let Some(tool_ref) = tool_ref else {
            logger.log(&format!("STEP9.{}: FAIL: tool not found in context", idx));
            records.push(TestToolCallRecord {
                tool_name,
                arguments: Value::Object(args),
                result: TestToolCallOutcome {
                    success: false,
                    content: Value::Null,
                    error: Some("tool not found in test context".into()),
                },
            });
            continue;
        };
        logger.log(&format!(
            "STEP9.{}: tool_ref found, kind={}",
            idx, tool_ref.kind
        ));

        let agent_ctx = Arc::new(AgentContext::new(
            "tool-test".into(),
            UserInput {
                raw_text: String::new(),
                session_id: None,
                message_id: None,
                timestamp: chrono::Utc::now(),
                metadata: std::collections::HashMap::new(),
            },
            ContextConfig::default(),
        ));
        let outcome = handle_workspace_tool(deps, &ctx, tool_ref, tc, 0, agent_ctx).await;
        logger.log(&format!(
            "STEP9.{}: handle_workspace_tool returned, payload_field_count={}",
            idx,
            outcome.payload.as_object().map_or(0, serde_json::Map::len)
        ));

        let (success, content, error) = match outcome {
            o if matches!(o.payload, Value::Object(_)) && !o.payload.get("error").is_some() => {
                (true, o.payload, None)
            }
            o => {
                let error = o
                    .payload
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                (false, o.payload, error)
            }
        };

        records.push(TestToolCallRecord {
            tool_name,
            arguments: Value::Object(args),
            result: TestToolCallOutcome {
                success,
                content,
                error,
            },
        });
    }

    logger.log(&format!(
        "DONE: returning {} tool_call_records",
        records.len()
    ));
    Ok(TestToolResult {
        assistant_content,
        has_tool_calls: true,
        tool_calls: records,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        DebugLogger, MAX_TRACE_ID_LEN, ToolTestError, ToolTestLlmRegistry,
        generate_tool_test_trace_id, resolve_tool_test_llm_target, tool_test_log_path,
    };
    use crate::runtime::execution_context::RuntimeExecutionContext;
    use crate::runtime::llm::LlmAdapterError;
    use async_trait::async_trait;
    use providers::{ChatRequest, LLMProvider, LLMResponse};
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct NeverCalledProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LLMProvider for NeverCalledProvider {
        fn default_model(&self) -> String {
            "global-default-must-not-run".to_string()
        }

        async fn chat(&self, _request: ChatRequest) -> LLMResponse {
            self.calls.fetch_add(1, Ordering::SeqCst);
            LLMResponse::default()
        }
    }

    struct UnknownPresetRegistry {
        provider: Arc<NeverCalledProvider>,
        build_calls: AtomicUsize,
    }

    impl ToolTestLlmRegistry for UnknownPresetRegistry {
        fn resolve_preset(
            &self,
            requested_preset: Option<&str>,
        ) -> Result<(String, u32, f32), LlmAdapterError> {
            Err(LlmAdapterError::Unknown(
                requested_preset.unwrap_or("global-default").to_string(),
            ))
        }

        fn build_chain(
            &self,
            _preset_name: Option<&str>,
        ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError> {
            self.build_calls.fetch_add(1, Ordering::SeqCst);
            Ok((
                Arc::clone(&self.provider) as Arc<dyn LLMProvider>,
                "global-default-model".to_string(),
            ))
        }
    }

    #[test]
    fn explicit_empty_and_unknown_tool_test_presets_are_typed_without_default_provider() {
        let provider = Arc::new(NeverCalledProvider {
            calls: AtomicUsize::new(0),
        });
        let registry = UnknownPresetRegistry {
            provider: Arc::clone(&provider),
            build_calls: AtomicUsize::new(0),
        };
        let execution_context =
            RuntimeExecutionContext::best_effort(Some("tool-test-unknown".into()), None).for_hook();

        for requested_preset in ["", "removed-preset"] {
            let error = match resolve_tool_test_llm_target(
                &registry,
                &execution_context,
                17,
                Some(requested_preset),
            ) {
                Ok(_) => panic!("explicit unknown preset must fail closed"),
                Err(error) => error,
            };
            assert!(matches!(
                error,
                ToolTestError::ModelPresetUnknown(name) if name == requested_preset
            ));
        }

        assert_eq!(registry.build_calls.load(Ordering::SeqCst), 0);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn generated_trace_id_has_fixed_safe_character_set() {
        let trace_id = generate_tool_test_trace_id();
        assert!(trace_id.len() <= MAX_TRACE_ID_LEN);
        assert!(
            trace_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        );
    }

    #[test]
    fn trace_id_rejects_path_traversal_and_unsafe_characters() {
        for unsafe_id in [
            "../escape",
            "..%2fescape",
            "nested/path",
            "nested\\path",
            "/absolute",
            "with space",
            "query?token=secret",
            "",
        ] {
            assert!(tool_test_log_path(unsafe_id).is_none(), "{unsafe_id}");
            assert!(
                DebugLogger::new(Some(unsafe_id.into()))
                    .trace_id()
                    .is_none()
            );
        }
        assert!(tool_test_log_path(&"a".repeat(MAX_TRACE_ID_LEN + 1)).is_none());
    }

    #[test]
    fn safe_trace_id_stays_inside_log_directory() {
        let trace_id = generate_tool_test_trace_id();
        let path = tool_test_log_path(&trace_id).expect("generated trace id must be safe");
        assert_eq!(path.parent(), Some(Path::new("/tmp/tool_test_logs")));
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(format!("{trace_id}.log").as_str())
        );
    }
}
