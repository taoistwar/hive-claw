//! Agent orchestrator — multi-hop tool-calling loop (T118 + T119 / US5 commit 2)
//!
//! 职责：
//!   1. 给定 agent_id + 用户消息历史 → 多轮 LLM 调用：每轮 LLM 可能要 tool_call
//!      → 宿主执行 → 把 result 当 tool message 再喂回去 → 直到无 tool_call 或达到上限
//!   2. 工具集 = DB agent.tools (kind=1 function-wrap / kind=2 workflow-wrap) +
//!      特殊工具 `route_to_subagent` (允许 main agent 切换到子 agent 处理)
//!   3. Hard-rule guards:
//!        - 路由 hop > AGENT_MAX_HOPS (default 5) → 终止 + emit error
//!        - 路由检测到 visited agent_id → 终止（spec FR-023 防循环）
//!        - depth > 10 → 终止
//!        - 子 agent 不继承父 permissions / preset（spec 安全模型）
//!   4. SSE event 出口：token 流来自 LLM；tool_call / tool_result / routed /
//!      fallback_used / done / error 由本模块编排
//!
//! 本模块**只发事件**：mpsc::Sender<Event> 由 chat 端点传入；模块完成后端点退出。

use aws_sdk_s3::Client as S3Client;
use axum::response::sse::Event;
use providers::{ChatRequest, RetryMode, ToolCallRequest};
use serde_json::{json, Value};
use sqlx::MySqlPool;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedSender;

use crate::runtime::capability::{CapabilityRegistry, DispatchCtx};
use crate::runtime::invoker::Invoker;
use crate::runtime::llm::LlmRegistry;
use crate::services::chat as chat_svc;
use crate::services::runtime_audit::{self, AuditRecord};

pub const ROUTE_TOOL_NAME: &str = "route_to_subagent";

fn max_hops() -> usize {
    std::env::var("AGENT_MAX_HOPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5)
}

/// 单次会话调用入口（spawned task）
pub struct OrchestratorDeps {
    pub pool: MySqlPool,
    pub s3: S3Client,
    pub llm: Arc<LlmRegistry>,
    pub registry: Arc<CapabilityRegistry>,
    pub invoker: Arc<Invoker>,
}

pub async fn run_session(
    deps: OrchestratorDeps,
    session_id: i64,
    starting_agent_id: i64,
    history: Vec<crate::models::ChatMessageAdmin>,
    user_content: String,
    tx: UnboundedSender<Result<Event, Infallible>>,
) {
    run_session_internal(
        deps, session_id, starting_agent_id, &history, &user_content, &tx, true,
    ).await;
}

pub async fn run_session_admin(
    deps: OrchestratorDeps,
    session_id: i64,
    starting_agent_id: i64,
    history: Vec<crate::models::ChatMessageAdmin>,
    user_content: String,
    tx: UnboundedSender<Result<Event, Infallible>>,
) {
    run_session_internal(
        deps, session_id, starting_agent_id, &history, &user_content, &tx, true,
    ).await;
}

async fn run_session_internal(
    deps: OrchestratorDeps,
    session_id: i64,
    starting_agent_id: i64,
    history: &[crate::models::ChatMessageAdmin],
    user_content: &str,
    tx: &UnboundedSender<Result<Event, Infallible>>,
    _is_admin: bool,
) {
    let elapsed_start = Instant::now();
    let mut current_agent_id = starting_agent_id;
    let mut visited: Vec<i64> = vec![starting_agent_id];
    let max_hops = max_hops();
    let mut final_content: Option<String> = None;
    let mut final_agent_id = starting_agent_id;

    // Create one Langfuse trace for the entire agent loop
    let lf_trace = providers::get_langfuse_client().map(|c| {
        let trace_input = json!({
            "session_id": session_id,
            "starting_agent_id": starting_agent_id,
            "max_hops": max_hops,
            "history_len": history.len(),
            "user_content": user_content,
        });
        c.trace("agent_loop", Some(trace_input))
    });

    // 把 history 转成 LLM-side messages（OpenAI-style）
    let mut messages: Vec<Value> = Vec::new();
    for m in history {
        if let Some(c) = &m.content {
            messages.push(json!({"role": m.role, "content": c}));
        }
    }
    messages.push(json!({"role": "user", "content": user_content}));

    for hop in 0..max_hops {
        // 1. 装配当前 agent 资源
        let ctx = match build_agent_context(&deps.pool, current_agent_id).await {
            Ok(c) => c,
            Err(e) => {
                emit_error(&tx, 5000, format!("agent context: {e}"));
                break;
            }
        };

        // 2. 构造 provider
        let (provider, model) = match deps.llm.build_primary(ctx.model_preset.as_deref()) {
            Ok(p) => p,
            Err(e) => {
                emit_error(&tx, 5007, format!("preset error: {e}"));
                break;
            }
        };

        // 3. 准备 system + tools
        let mut hop_msgs: Vec<Value> = vec![json!({"role": "system", "content": ctx.system_prompt})];
        hop_msgs.extend(messages.clone());

        let tools_schema = build_tools_schema(&ctx);
        let req = ChatRequest {
            model: Some(model.clone()),
            messages: hop_msgs,
            max_tokens: 2048,
            temperature: 0.7,
            tools: if tools_schema.is_empty() {
                None
            } else {
                Some(tools_schema.clone())
            },
            tool_choice: None,
            reasoning_effort: None,
        };

        // 4. stream LLM with on_delta token forwarding
        let tx_inner = tx.clone();
        let on_delta: providers::StreamDeltaCallback = Arc::new(move |delta: String| {
            let payload = json!({ "text": delta });
            let ev = Event::default().event("token").data(payload.to_string());
            let _ = tx_inner.send(Ok::<_, Infallible>(ev));
        });
        let resp = provider.chat_stream_with_retry(req, Some(on_delta), None, RetryMode::Standard, None, lf_trace.as_ref()).await;

        if resp.is_error() {
            let msg = resp
                .content
                .clone()
                .or(resp.error_kind.clone())
                .unwrap_or_else(|| "LLM error".into());
            emit_error(&tx, resp.error_status_code.unwrap_or(5000) as u16, msg);
            audit_llm(&deps.pool, current_agent_id, "error", elapsed_start, &model).await;
            break;
        }

        // assistant content + tool_calls 都加入 messages，下一轮接 tool messages
        let assistant_content = resp.content.clone().unwrap_or_default();
        let tool_calls = resp.tool_calls.clone();

        // 把 assistant 消息加入 history（含 tool_calls 序列化）
        if !tool_calls.is_empty() {
            let tc_json: Vec<Value> = tool_calls.iter().map(|tc| tc.to_openai_tool_call()).collect();
            messages.push(json!({
                "role": "assistant",
                "content": assistant_content.clone(),
                "tool_calls": tc_json,
            }));
        } else {
            messages.push(json!({"role": "assistant", "content": assistant_content.clone()}));
        }

        audit_llm(&deps.pool, current_agent_id, "success", elapsed_start, &model).await;

        // 5. 无 tool_call → 这是最终回复
        if !resp.should_execute_tools() {
            final_content = Some(assistant_content);
            final_agent_id = current_agent_id;
            break;
        }

        // 6. 处理 tool_calls
        let mut routed_to: Option<i64> = None;
        for tc in &tool_calls {
            // emit tool_call event
            let tc_payload = json!({
                "tool_call_id": tc.id,
                "name": tc.name,
                "args": tc.arguments,
            });
            let _ = tx.send(Ok(Event::default().event("tool_call").data(tc_payload.to_string())));

            let result = if tc.name == ROUTE_TOOL_NAME {
                handle_route_tool(&deps.pool, &ctx, &visited, tc).await
            } else if let Some(tool_ref) = ctx.tools.iter().find(|t| t.identifier == tc.name) {
                handle_workspace_tool(&deps, &ctx, tool_ref, tc, session_id).await
            } else {
                ToolOutcome::error(format!("未知工具：{}", tc.name))
            };

            // emit tool_result event
            let tr_payload = json!({
                "tool_call_id": tc.id,
                "result": result.payload,
            });
            let _ = tx.send(Ok(Event::default().event("tool_result").data(tr_payload.to_string())));

            // tool message → 加入 history
            messages.push(json!({
                "role": "tool",
                "tool_call_id": tc.id,
                "content": serde_json::to_string(&result.payload).unwrap_or_default(),
            }));

            if let Some(next_agent) = result.route_to {
                if visited.contains(&next_agent) {
                    emit_error(
                        &tx,
                        5006,
                        format!("路由循环检测：agent_id={} 已访问过", next_agent),
                    );
                    final_content = Some(assistant_content.clone());
                    final_agent_id = current_agent_id;
                    return finalize(&deps.pool, session_id, &tx, elapsed_start, final_content, final_agent_id).await;
                }
                routed_to = Some(next_agent);
                visited.push(next_agent);
                // emit routed event
                let routed_payload = json!({
                    "agent_id": next_agent,
                    "agent_identifier": result.route_identifier.clone().unwrap_or_default(),
                });
                let _ = tx.send(Ok(Event::default().event("routed").data(routed_payload.to_string())));
                audit_route(&deps.pool, current_agent_id, next_agent).await;
            }
        }

        if let Some(next) = routed_to {
            current_agent_id = next;
            // 路由后下一轮继续；保持 messages 累积让新 agent 看到上下文
            continue;
        }

        // 有 tool_calls 但没路由 → 下一轮 LLM 用 tool result 继续
        if hop + 1 == max_hops {
            emit_error(&tx, 5006, format!("已达最大 hop {max_hops}"));
            final_content = Some(assistant_content);
            final_agent_id = current_agent_id;
            break;
        }
    }

    finalize(&deps.pool, session_id, &tx, elapsed_start, final_content, final_agent_id).await;
}

async fn finalize(
    pool: &MySqlPool,
    session_id: i64,
    tx: &UnboundedSender<Result<Event, Infallible>>,
    started: Instant,
    content: Option<String>,
    final_agent_id: i64,
) {
    let elapsed = started.elapsed().as_millis() as i32;
    if let Some(text) = content {
        let routed = if final_agent_id != 1 { Some(final_agent_id) } else { None };
        let _ = chat_svc::append_assistant_message_admin(pool, session_id, &text, routed, Some(elapsed)).await;
    }
    let done = json!({
        "elapsed_ms": elapsed,
        "final_agent_id": if final_agent_id != 1 { Some(final_agent_id) } else { None },
    });
    let _ = tx.send(Ok(Event::default().event("done").data(done.to_string())));
}

fn emit_error(tx: &UnboundedSender<Result<Event, Infallible>>, code: u16, message: String) {
    let _ = tx.send(Ok(Event::default()
        .event("error")
        .data(json!({"code": code, "message": message}).to_string())));
}

// ============================ Agent context ============================

#[derive(Debug, Clone)]
pub(crate) struct AgentContext {
    pub(crate) agent_id: i64,
    pub(crate) identifier: String,
    pub(crate) system_prompt: String,
    pub(crate) model_preset: Option<String>,
    pub(crate) tools: Vec<ToolRef>,
    pub(crate) permissions: Vec<String>,
    pub(crate) children: Vec<ChildAgent>,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolRef {
    pub(crate) id: i64,
    pub(crate) identifier: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) kind: i8, // 1 function-wrap, 2 workflow-wrap
    pub(crate) function_id: Option<i64>,
    pub(crate) workflow_id: Option<i64>,
    pub(crate) input_schema: Value,
    /// custom function 对应的 plugin_id + plugin_export（kind=1 时填充）
    pub(crate) plugin_id: Option<i64>,
    pub(crate) plugin_export: Option<String>,
    /// builtin function（plugin_id IS NULL）标记
    pub(crate) is_builtin_function: bool,
    /// 元工具：kind=1 且 function_id=NULL（如 invoke_function / invoke_workflow）
    pub(crate) is_meta_tool: bool,
    /// 该 tool/function 声明所需的 capabilities
    pub(crate) required_capabilities: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ChildAgent {
    pub(crate) id: i64,
    pub(crate) identifier: String,
    pub(crate) description: Option<String>,
}

async fn build_agent_context(pool: &MySqlPool, agent_id: i64) -> Result<AgentContext, String> {
    let row: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT identifier, system_prompt, model_preset FROM agents WHERE id = ?",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("agent fetch: {e}"))?;
    let (identifier, mut system_prompt, model_preset) = row.ok_or_else(|| {
        format!("agent id={agent_id} not found")
    })?;

    // Skill markdown 拼到 system prompt
    let skills: Vec<(String,)> = sqlx::query_as(
        r#"SELECT s.content FROM skills s
           JOIN agent_skills ax ON ax.skill_id = s.id
           WHERE ax.agent_id = ?"#,
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    for (md,) in &skills {
        system_prompt.push_str("\n\n");
        system_prompt.push_str(md);
    }

    // Tools: agent-specific + always tools (deduplicated by tool id)
    let tool_rows: Vec<(i64, String, String, String, i8, Option<i64>, Option<i64>, Value, Option<i64>, Option<String>, Option<Value>)> =
        sqlx::query_as(
            r#"SELECT t.id, t.identifier, t.name, t.description, t.kind,
                      t.function_id, t.workflow_id, t.input_schema,
                      f.plugin_id, f.plugin_export,
                      COALESCE(t.required_capabilities, f.required_capabilities)
               FROM tools t
               JOIN agent_tools at ON at.tool_id = t.id
               LEFT JOIN functions f ON f.id = t.function_id
               WHERE at.agent_id = ?
               UNION
               SELECT t.id, t.identifier, t.name, t.description, t.kind,
                      t.function_id, t.workflow_id, t.input_schema,
                      f.plugin_id, f.plugin_export,
                      COALESCE(t.required_capabilities, f.required_capabilities)
               FROM tools t
               LEFT JOIN functions f ON f.id = t.function_id
               WHERE t.is_always = 1
                 AND t.id NOT IN (
                     SELECT at2.tool_id FROM agent_tools at2 WHERE at2.agent_id = ?
                 )"#,
        )
        .bind(agent_id)
        .bind(agent_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

    let mut tools: Vec<ToolRef> = Vec::new();
    for (id, identifier, name, desc, kind, function_id, workflow_id, input_schema, plugin_id, plugin_export, caps_json) in tool_rows {
        let is_builtin_function = kind == 1 && plugin_id.is_none();
        // 元工具（meta-tool）：kind=1 且 function_id=NULL（如 invoke_function / invoke_workflow）
        let is_meta_tool = kind == 1 && function_id.is_none();
        let required_capabilities: Vec<String> = caps_json
            .and_then(|v| v.as_array().cloned())
            .map(|arr| arr.into_iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        tools.push(ToolRef {
            id,
            identifier,
            name,
            description: desc,
            kind,
            function_id,
            workflow_id,
            input_schema,
            plugin_id,
            plugin_export,
            is_builtin_function,
            is_meta_tool,
            required_capabilities,
        });
    }

    // Permissions
    let perms: Vec<(String,)> =
        sqlx::query_as("SELECT capability FROM agent_permissions WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default();

    // Children
    let children_rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT id, identifier, description FROM agents WHERE parent_agent_id = ?",
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let children: Vec<ChildAgent> = children_rows
        .into_iter()
        .map(|(id, ident, desc)| ChildAgent { id, identifier: ident, description: desc })
        .collect();

    Ok(AgentContext {
        agent_id,
        identifier,
        system_prompt,
        model_preset,
        tools,
        permissions: perms.into_iter().map(|(c,)| c).collect(),
        children,
    })
}

/// 组装 LLM-side tools schema（OpenAI function-calling format）
pub(crate) fn build_tools_schema(ctx: &AgentContext) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    // route_to_subagent（仅当存在子 agent 时暴露）
    if !ctx.children.is_empty() {
        let enum_vals: Vec<Value> = ctx
            .children
            .iter()
            .map(|c| Value::String(c.identifier.clone()))
            .collect();
        let desc = format!(
            "Switch to a sub-agent for specialized handling. Available sub-agents:\n{}",
            ctx.children
                .iter()
                .map(|c| format!(
                    "- {}: {}",
                    c.identifier,
                    c.description.as_deref().unwrap_or("")
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );
        out.push(json!({
            "type": "function",
            "function": {
                "name": ROUTE_TOOL_NAME,
                "description": desc,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "agent_identifier": {
                            "type": "string",
                            "enum": enum_vals,
                            "description": "The identifier of the sub-agent to route to",
                        },
                        "reason": {
                            "type": "string",
                            "description": "Why this sub-agent is being chosen",
                        },
                    },
                    "required": ["agent_identifier"],
                },
            }
        }));
    }
    // workspace tools
    for t in &ctx.tools {
        out.push(json!({
            "type": "function",
            "function": {
                "name": t.identifier,
                "description": t.description,
                "parameters": t.input_schema,
            }
        }));
    }
    out
}

/// 简化版：仅从 ToolRef 列表构建 schema（用于 tool_test）
pub(crate) fn build_tools_schema_simple(tools: &[ToolRef]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.identifier,
                    "description": t.description,
                    "parameters": t.input_schema,
                }
            })
        })
        .collect()
}

// ============================ Tool execution ============================

#[derive(Debug, Clone)]
pub(crate) struct ToolOutcome {
    pub(crate) payload: Value,
    route_to: Option<i64>,
    route_identifier: Option<String>,
}

impl ToolOutcome {
    pub(crate) fn ok(payload: Value) -> Self {
        Self { payload, route_to: None, route_identifier: None }
    }
    fn route(agent_id: i64, identifier: String) -> Self {
        Self {
            payload: json!({"routed": true, "agent_id": agent_id, "agent_identifier": identifier}),
            route_to: Some(agent_id),
            route_identifier: Some(identifier),
        }
    }
    pub(crate) fn error(msg: String) -> Self {
        Self {
            payload: json!({"error": msg}),
            route_to: None,
            route_identifier: None,
        }
    }
}

async fn handle_route_tool(
    pool: &MySqlPool,
    parent_ctx: &AgentContext,
    visited: &[i64],
    tc: &ToolCallRequest,
) -> ToolOutcome {
    let target_ident = tc.arguments
        .get("agent_identifier")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if target_ident.is_empty() {
        return ToolOutcome::error("agent_identifier 缺失".into());
    }
    // 必须是 parent 的直接子 agent
    let child = parent_ctx.children.iter().find(|c| c.identifier == target_ident);
    let Some(child) = child else {
        return ToolOutcome::error(format!(
            "agent「{target_ident}」不是当前 agent「{}」的子 agent，路由拒绝",
            parent_ctx.identifier
        ));
    };
    if visited.contains(&child.id) {
        return ToolOutcome::error(format!(
            "agent_id={} 已在本会话访问过，循环路由拒绝",
            child.id
        ));
    }
    // depth 检查由 agents.chk_agents_depth 在创建时已保证 ≤ 10
    let _ = pool; // not used here; reserved for future audit hooks
    ToolOutcome::route(child.id, child.identifier.clone())
}

async fn handle_meta_tool(
    deps: &OrchestratorDeps,
    ctx: &AgentContext,
    tool_ref: &ToolRef,
    tc: &ToolCallRequest,
    session_id: i64,
) -> ToolOutcome {
    match tool_ref.identifier.as_str() {
        "invoke_function" => {
            let func_ident = match tc.arguments.get("function_identifier").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return ToolOutcome::error("invoke_function: function_identifier 缺失".into()),
            };
            let function_input = match tc.arguments.get("function_input") {
                Some(v) => v.clone(),
                None => return ToolOutcome::error("invoke_function: function_input 缺失".into()),
            };
            // 查询 function 信息（包含 required_capabilities）
            let func_row: Option<(i64, i8, Option<i64>, Option<String>, Option<Value>)> = sqlx::query_as(
                "SELECT id, kind, plugin_id, plugin_export, required_capabilities FROM functions WHERE identifier = ?",
            )
            .bind(&func_ident)
            .fetch_optional(&deps.pool)
            .await
            .map_err(|e| format!("function lookup: {e}"))
            .unwrap_or(None);
            let Some((func_id, func_kind, plugin_id, plugin_export, func_caps)) = func_row else {
                return ToolOutcome::error(format!("invoke_function: function「{func_ident}」不存在"));
            };

            // Capability check: 如果 function 声明了 required_capabilities，校验 agent 权限
            if let Some(ref caps) = func_caps {
                if let Some(arr) = caps.as_array() {
                    let required: Vec<String> = arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    if !required.is_empty() {
                        let agent_perms: std::collections::HashSet<&str> = ctx.permissions.iter().map(|s| s.as_str()).collect();
                        let missing: Vec<&str> = required.iter()
                            .filter(|c| !agent_perms.contains(c.as_str()))
                            .map(|s| s.as_str())
                            .collect();
                        if !missing.is_empty() {
                            return ToolOutcome::error(format!(
                                "function「{func_ident}」需要能力「{}」，但当前 Agent 未授权",
                                missing.join("、")
                            ));
                        }
                    }
                }
            }

            match func_kind {
                1 if plugin_id.is_none() => {
                    // builtin function — 直接调用
                    let Some(builtin) = super::builtins::lookup(&func_ident) else {
                        return ToolOutcome::error(format!("builtin function「{func_ident}」未找到 handler"));
                    };
                    match (builtin.handler)(function_input) {
                        Ok(result) => ToolOutcome::ok(result),
                        Err(e) => ToolOutcome::error(format!("builtin function 执行失败: {e}")),
                    }
                }
                1 | 2 => {
                    // custom function (kind=2) 或 plugin-based function (kind=1) — 通过 invoker 调用
                    let Some(pid) = plugin_id else {
                        return ToolOutcome::error("function 缺 plugin_id".into());
                    };
                    let Some(ref export) = plugin_export else {
                        return ToolOutcome::error("function 缺 plugin_export".into());
                    };
                    let input_json = match serde_json::to_string(&function_input) {
                        Ok(s) => s,
                        Err(e) => return ToolOutcome::error(format!("args serialize: {e}")),
                    };
                    let dispatch_ctx = DispatchCtx {
                        request_id: None,
                        session_id: Some(session_id),
                        agent_id: ctx.agent_id,
                        plugin_id: pid,
                        function_id: Some(func_id),
                        permissions: ctx.permissions.clone(),
                    };
                    match deps.invoker.invoke(
                        &deps.pool, &deps.s3,
                        Arc::clone(&deps.registry), Arc::clone(&deps.llm),
                        pid, export, input_json, dispatch_ctx,
                    ).await {
                        Ok(out_str) => {
                            let parsed: Value = serde_json::from_str(&out_str)
                                .unwrap_or_else(|_| Value::String(out_str));
                            ToolOutcome::ok(parsed)
                        }
                        Err(e) => ToolOutcome::error(format!("plugin invoke failed: {e}")),
                    }
                }
                _ => ToolOutcome::error(format!("invoke_function: function「{func_ident}」kind={func_kind} 不支持")),
            }
        }
        "invoke_workflow" => {
            let wf_ident = match tc.arguments.get("workflow_identifier").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return ToolOutcome::error("invoke_workflow: workflow_identifier 缺失".into()),
            };
            let workflow_input = match tc.arguments.get("workflow_input") {
                Some(v) => v.clone(),
                None => return ToolOutcome::error("invoke_workflow: workflow_input 缺失".into()),
            };
            // 查询 workflow id
            let wf_row: Option<(i64,)> = sqlx::query_as(
                "SELECT id FROM workflows WHERE identifier = ?",
            )
            .bind(&wf_ident)
            .fetch_optional(&deps.pool)
            .await
            .map_err(|e| format!("workflow lookup: {e}"))
            .unwrap_or(None);
            let Some((workflow_id,)) = wf_row else {
                return ToolOutcome::error(format!("invoke_workflow: workflow「{wf_ident}」不存在"));
            };
            let executor_deps = crate::runtime::workflow::ExecutorDeps {
                pool: deps.pool.clone(),
                s3: deps.s3.clone(),
                registry: Arc::clone(&deps.registry),
                llm: Arc::clone(&deps.llm),
                invoker: Arc::clone(&deps.invoker),
            };
            let executor = crate::runtime::workflow::WorkflowExecutor::new();
            match executor.execute(&executor_deps, workflow_id, workflow_input, ctx.agent_id).await {
                Ok(out) => {
                    let obj = serde_json::Map::from_iter(out.into_iter());
                    ToolOutcome::ok(Value::Object(obj))
                }
                Err(e) => ToolOutcome::error(format!("workflow execute: {e}")),
            }
        }
        _ => ToolOutcome::error(format!("未知元工具: {}", tool_ref.identifier)),
    }
}

pub(crate) async fn handle_workspace_tool(
    deps: &OrchestratorDeps,
    ctx: &AgentContext,
    tool_ref: &ToolRef,
    tc: &ToolCallRequest,
    session_id: i64,
) -> ToolOutcome {
    // Capability check: 如果 tool 声明了 required_capabilities，校验 agent 权限
    if !tool_ref.required_capabilities.is_empty() {
        let agent_perms: std::collections::HashSet<&str> = ctx.permissions.iter().map(|s| s.as_str()).collect();
        let missing: Vec<&str> = tool_ref.required_capabilities.iter()
            .filter(|c| !agent_perms.contains(c.as_str()))
            .map(|s| s.as_str())
            .collect();
        if !missing.is_empty() {
            return ToolOutcome::error(format!(
                "tool「{}」需要能力「{}」，但当前 Agent 未授权",
                tool_ref.identifier,
                missing.join("、")
            ));
        }
    }

    match tool_ref.kind {
        1 => {
            if tool_ref.is_meta_tool {
                // 元工具 — 根据 identifier 分发到 invoke_function / invoke_workflow
                handle_meta_tool(deps, ctx, tool_ref, tc, session_id).await
            } else if tool_ref.is_builtin_function {
                if tool_ref.function_id.is_none() {
                    return ToolOutcome::error("builtin function 缺 function_id".into());
                };
                let Some(builtin) = super::builtins::lookup(&tool_ref.identifier) else {
                    return ToolOutcome::error(format!("builtin function「{}」未找到 handler", tool_ref.identifier));
                };
                let args_value: Value = Value::Object(tc.arguments.clone());
                match (builtin.handler)(args_value) {
                    Ok(result) => ToolOutcome::ok(result),
                    Err(e) => ToolOutcome::error(format!("builtin function 执行失败: {e}")),
                }
            } else {
                // function-wrap → invoker.invoke(plugin_id, plugin_export, args)
                let Some(plugin_id) = tool_ref.plugin_id else {
                    return ToolOutcome::error("function 缺 plugin_id".into());
                };
                let Some(ref export) = tool_ref.plugin_export else {
                    return ToolOutcome::error("function 缺 plugin_export".into());
                };
                let args_value: Value = Value::Object(tc.arguments.clone());
                let input_json = match serde_json::to_string(&args_value) {
                    Ok(s) => s,
                    Err(e) => return ToolOutcome::error(format!("args serialize: {e}")),
                };
                let dispatch_ctx = DispatchCtx {
                    request_id: None,
                    session_id: Some(session_id),
                    agent_id: ctx.agent_id,
                    plugin_id,
                    function_id: tool_ref.function_id,
                    permissions: ctx.permissions.clone(),
                };
                match deps
                    .invoker
                    .invoke(
                        &deps.pool,
                        &deps.s3,
                        Arc::clone(&deps.registry),
                        Arc::clone(&deps.llm),
                        plugin_id,
                        export,
                        input_json,
                        dispatch_ctx,
                    )
                    .await
                {
                    Ok(out_str) => {
                        // Plugin 返回的是 JSON 字符串 — 解析后封装；失败 → 当作 string
                        let parsed: Value = serde_json::from_str(&out_str)
                            .unwrap_or_else(|_| Value::String(out_str));
                        ToolOutcome::ok(parsed)
                    }
                    Err(e) => ToolOutcome::error(format!("plugin invoke failed: {e}")),
                }
            }
        }
        2 => {
            // workflow-wrap (T111) — 直接派发到 WorkflowExecutor
            let Some(workflow_id) = tool_ref.workflow_id else {
                return ToolOutcome::error("workflow-wrap tool 缺 workflow_id".into());
            };
            let args_value: Value = Value::Object(tc.arguments.clone());
            let executor_deps = crate::runtime::workflow::ExecutorDeps {
                pool: deps.pool.clone(),
                s3: deps.s3.clone(),
                registry: Arc::clone(&deps.registry),
                llm: Arc::clone(&deps.llm),
                invoker: Arc::clone(&deps.invoker),
            };
            // 临时构造 executor — 直接用 sentinel；workflows 持有也行
            let executor = crate::runtime::workflow::WorkflowExecutor::new();
            match executor
                .execute(&executor_deps, workflow_id, args_value, ctx.agent_id)
                .await
            {
                Ok(out) => {
                    // 把 HashMap<node_key, Value> 当成对象返回
                    let obj = serde_json::Map::from_iter(out.into_iter());
                    ToolOutcome::ok(Value::Object(obj))
                }
                Err(e) => ToolOutcome::error(format!("workflow execute: {e}")),
            }
        }
        _ => ToolOutcome::error(format!("未知 tool kind: {}", tool_ref.kind)),
    }
}

// ============================ Audit ============================

async fn audit_llm(
    pool: &MySqlPool,
    agent_id: i64,
    outcome: &str,
    started: Instant,
    model: &str,
) {
    let _ = model;
    runtime_audit::record(
        pool,
        AuditRecord {
            request_id: None,
            session_id: None,
            agent_id: Some(agent_id),
            plugin_id: None,
            function_id: None,
            capability: None,
            event_type: "llm_invoke",
            outcome,
            elapsed_ms: Some(started.elapsed().as_millis() as i32),
            error_message: None,
            payload_summary: None,
        },
    )
    .await;
}

async fn audit_route(pool: &MySqlPool, from: i64, to: i64) {
    runtime_audit::record(
        pool,
        AuditRecord {
            request_id: None,
            session_id: None,
            agent_id: Some(from),
            plugin_id: None,
            function_id: None,
            capability: None,
            event_type: "agent_route",
            outcome: "success",
            elapsed_ms: None,
            error_message: None,
            payload_summary: Some(json!({"to_agent_id": to})),
        },
    )
    .await;
}
