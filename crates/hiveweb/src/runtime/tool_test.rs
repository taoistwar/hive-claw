//! Tool test runner (T127 / US7)
//!
//! 为单个 Tool 创建隔离测试环境：加载目标 Tool + Always Tools + Always Skills → 执行单轮对话 → 返回结果

use providers::{ChatRequest, RetryMode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::MySqlPool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use agent::context::{AgentContext, ContextConfig, UserInput};
use super::orchestrator::{
    AgentContent, OrchestratorDeps, ToolRef, build_tools_schema_simple, handle_workspace_tool,
};
use crate::services::runtime_audit::{self, AuditRecord};

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

#[derive(Clone)]
pub struct DebugLogger {
    trace_id: Option<String>,
    lines: Arc<Mutex<Vec<String>>>,
}

impl DebugLogger {
    pub fn new(trace_id: Option<String>) -> Self {
        Self {
            trace_id,
            lines: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn log(&self, msg: &str) {
        let line = format!("[tool_test] {}", msg);
        let is_dev = std::env::var("APP_ENV")
            .map(|v| v == "development" || v == "dev")
            .unwrap_or(true);
        if is_dev {
            if let Some(ref tid) = self.trace_id {
                let _ = std::fs::create_dir_all("/tmp/tool_test_logs");
                let path = format!("/tmp/tool_test_logs/{}.log", tid);
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .and_then(|mut f| {
                        use std::io::Write;
                        writeln!(f, "{}", line)
                    });
            }
        }
        if let Ok(mut lines) = self.lines.lock() {
            lines.push(line.clone());
        }
        println!("{}", line);
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
) -> Result<TestToolResult, String> {
    let logger = DebugLogger::new(req.trace_id.clone());
    logger.log(&format!(
        "START tool_id={} message={}",
        tool_id, req.message
    ));
    // 1. 查询目标 tool
    let tool_row: Option<(
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
    )> = sqlx::query_as(
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
        return Err(format!("tool id={tool_id} not found"));
    };
    logger.log(&format!(
        "STEP1 OK: tool found id={} identifier={} kind={}",
        tid, t_ident, t_kind
    ));

    let t_caps: Vec<String> = t_caps_raw
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    // 2. 加载 always tools（排除目标 tool 避免重复）
    let always_tools: Vec<(
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
    )> = sqlx::query_as(
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
    let (provider, model) = deps
        .llm
        .build_primary(ctx.model_preset.as_deref())
        .map_err(|e| format!("LLM provider: {e}"))?;
    logger.log(&format!("STEP5 OK: LLM provider built, model={}", model));

    let tools_schema = build_tools_schema_simple(&ctx.tools);

    let messages: Vec<Value> = vec![
        json!({"role": "system", "content": ctx.system_prompt}),
        json!({"role": "user", "content": req.message}),
    ];

    let chat_req = ChatRequest {
        model: Some(model),
        messages,
        max_tokens: 4096,
        temperature: 0.7,
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

    let llm_started = Instant::now();
    logger.log("STEP7: calling chat_stream_with_retry...");
    let resp = provider
        .chat_stream_with_retry(chat_req, None, None, RetryMode::Standard, None)
        .await;
    let llm_elapsed_ms = llm_started.elapsed().as_millis() as i32;
    logger.log(&format!(
        "STEP7 DONE: LLM responded in {}ms, is_error={}, content_len={}",
        llm_elapsed_ms,
        resp.is_error(),
        resp.content.as_ref().map(|s| s.len()).unwrap_or(0)
    ));

    if resp.is_error() {
        let err_msg = resp.content.clone().unwrap_or_else(|| "unknown".into());
        logger.log(&format!("FAIL: LLM error: {}", err_msg));
        runtime_audit::record(
            pool,
            AuditRecord {
                request_id: None,
                session_id: None,
                agent_id: None,
                plugin_id: None,
                function_id: None,
                capability: None,
                event_type: "llm_invoke",
                outcome: "error",
                elapsed_ms: Some(llm_elapsed_ms),
                error_message: Some(&err_msg),
                payload_summary: Some(json!({"mode": "tool_test", "tool_id": tool_id})),
            },
        )
        .await;
        return Err(format!("LLM error: {}", err_msg));
    }

    runtime_audit::record(
        pool,
        AuditRecord {
            request_id: None,
            session_id: None,
            agent_id: None,
            plugin_id: None,
            function_id: None,
            capability: None,
            event_type: "llm_invoke",
            outcome: "success",
            elapsed_ms: Some(llm_elapsed_ms),
            error_message: None,
            payload_summary: Some(json!({"mode": "tool_test", "tool_id": tool_id})),
        },
    )
    .await;
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
            "STEP9.{}: executing tool '{}' with args={}",
            idx,
            tool_name,
            serde_json::to_string(&args).unwrap_or_default()
        ));

        let tool_ref = ctx.tools.iter().find(|t| t.identifier == tc.name);
        let Some(tool_ref) = tool_ref else {
            logger.log(&format!(
                "STEP9.{}: FAIL: tool '{}' not found in context",
                idx, tc.name
            ));
            records.push(TestToolCallRecord {
                tool_name,
                arguments: Value::Object(args),
                result: TestToolCallOutcome {
                    success: false,
                    content: Value::Null,
                    error: Some(format!("tool '{}' not found in test context", tc.name)),
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
            "STEP9.{}: handle_workspace_tool returned, payload_keys={:?}",
            idx,
            outcome
                .payload
                .as_object()
                .map(|o| o.keys().collect::<Vec<_>>())
                .unwrap_or_default()
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
