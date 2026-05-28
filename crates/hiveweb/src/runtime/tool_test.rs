//! Tool test runner (T127 / US7)
//!
//! 为单个 Tool 创建隔离测试环境：加载目标 Tool + Always Tools + Always Skills → 执行单轮对话 → 返回结果

use providers::{ChatRequest, LLMProvider, RetryMode, ToolCallRequest};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::MySqlPool;

use super::orchestrator::{
    build_tools_schema_simple, handle_workspace_tool, AgentContext, OrchestratorDeps, ToolRef, ToolOutcome,
};

#[derive(Debug, Deserialize)]
pub struct TestToolRequest {
    pub message: String,
    pub model_preset: Option<String>,
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

pub async fn run_tool_test(
    pool: &MySqlPool,
    deps: &OrchestratorDeps,
    tool_id: i64,
    req: TestToolRequest,
) -> Result<TestToolResult, String> {
    // 1. 查询目标 tool
    let tool_row: Option<(i64, String, String, String, i8, Option<i64>, Option<i64>, Value, Option<i64>, Option<String>, Option<String>)> =
        sqlx::query_as(
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

    let Some((tid, t_ident, t_name, t_desc, t_kind, t_fid, t_wid, t_input_schema, t_plugin_id, t_plugin_export, t_caps_raw)) = tool_row else {
        return Err(format!("tool id={tool_id} not found"));
    };

    let t_caps: Vec<String> = t_caps_raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    // 2. 加载 always tools（排除目标 tool 避免重复）
    let always_tools: Vec<(i64, String, String, String, i8, Option<i64>, Option<i64>, Value, Option<i64>, Option<String>, Option<String>)> =
        sqlx::query_as(
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

    // 3. 构建 ToolRef 列表
    let mut tools: Vec<ToolRef> = Vec::new();
    // 目标 tool
    tools.push(ToolRef {
        id: tid,
        identifier: t_ident,
        name: t_name,
        description: t_desc,
        kind: t_kind,
        function_id: t_fid,
        workflow_id: t_wid,
        input_schema: t_input_schema,
        plugin_id: t_plugin_id,
        plugin_export: t_plugin_export,
        is_builtin_function: t_kind == 1 && t_plugin_id.is_none(),
        is_meta_tool: t_kind == 1 && t_fid.is_none(),
        required_capabilities: t_caps,
    });
    // always tools
    for (id, ident, name, desc, kind, fid, wid, input_schema, pid, pexport, caps_raw) in always_tools {
        let is_builtin = kind == 1 && pid.is_none();
        let is_meta = kind == 1 && fid.is_none();
        let required_capabilities: Vec<String> = caps_raw
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        tools.push(ToolRef {
            id,
            identifier: ident,
            name,
            description: desc,
            kind,
            function_id: fid,
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
    let skills: Vec<(String,)> = sqlx::query_as(
        "SELECT content FROM skills WHERE is_always = 1",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("always skills: {e}"))?;

    let mut system_prompt = "You are a test assistant. Use the available tools when appropriate.".to_string();
    for (md,) in &skills {
        system_prompt.push_str("\n\n--- SKILL ---\n\n");
        system_prompt.push_str(md);
    }

    // 测试模式：授予所有 capability 权限，以便插件能自由调用 network.http 等能力
    let permissions: Vec<String> = crate::runtime::capability::CAPABILITIES
        .iter()
        .map(|c| c.name.to_string())
        .collect();

    let ctx = AgentContext {
        agent_id: 0, // 测试模式，不需要真实 agent_id
        identifier: format!("test_tool_{}", tool_id),
        system_prompt,
        model_preset: req.model_preset,
        tools,
        permissions,
        children: vec![],
    };

    // 5. 构建 LLM request
    let (provider, model) = deps.llm.build_primary(ctx.model_preset.as_deref())
        .map_err(|e| format!("LLM provider: {e}"))?;

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
        tools: if tools_schema.is_empty() { None } else { Some(tools_schema) },
        tool_choice: None,
        reasoning_effort: None,
    };

    let resp = provider.chat_stream_with_retry(chat_req, None, None, RetryMode::Standard, None).await;

    if resp.is_error() {
        return Err(format!("LLM error: {}", resp.content.unwrap_or_else(|| "unknown".into())));
    }

    let assistant_content = resp.content.unwrap_or_default();
    let tool_calls = resp.tool_calls;

    if tool_calls.is_empty() {
        return Ok(TestToolResult {
            assistant_content,
            has_tool_calls: false,
            tool_calls: vec![],
        });
    }

    // 6. 执行工具调用
    let mut records = Vec::new();
    for tc in tool_calls {
        let tool_name = tc.name.clone();
        let args = tc.arguments.clone();

        let tool_ref = ctx.tools.iter().find(|t| t.identifier == tc.name);
        let Some(tool_ref) = tool_ref else {
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

        let outcome = handle_workspace_tool(deps, &ctx, tool_ref, &tc, 0).await;

        let (success, content, error) = match outcome {
            o if matches!(o.payload, Value::Object(_)) && !o.payload.get("error").is_some() => {
                (true, o.payload, None)
            }
            o => {
                let error = o.payload.get("error").and_then(|v| v.as_str()).map(|s| s.to_string());
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

    Ok(TestToolResult {
        assistant_content,
        has_tool_calls: true,
        tool_calls: records,
    })
}
