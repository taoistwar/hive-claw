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
use chrono::Utc;
use providers::{ChatRequest, RetryMode, ToolCallRequest};
use serde_json::{Value, json};
use sqlx::MySqlPool;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedSender;

use agent::context::{
    AgentContext, Category, ContextConfig, ExtensionContent, LifecycleState, ResponsePayload,
    ToolCallStatus, UserInput,
};
use crate::runtime::capability::{CapabilityRegistry, DispatchCtx};
use crate::runtime::invoker::Invoker;
use crate::runtime::llm::LlmRegistry;
use crate::runtime::hook::{self, apply_agent_context_updates, inject_agent_context_snapshot, HookContext, HookDeps};
use crate::services::chat_user::append_assistant_message_user;
use crate::services::runtime_audit::{self, AuditRecord};
use crate::models::ChatMessageUser;

pub const ROUTE_TOOL_NAME: &str = "route_to_subagent";

fn max_hops() -> usize {
    std::env::var("AGENT_MAX_HOPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5)
}

/// 将 ExtensionContent 扁平化为 { content_type, ...data_fields } 格式，
/// 去掉 id / reply / render_hints / data 等包装层。
fn flatten_extension(e: &ExtensionContent) -> Value {
    let mut flat = serde_json::Map::new();
    flat.insert(
        "content_type".to_string(),
        json!(e.content_type.to_string()),
    );
    if let Value::Object(data_obj) = &e.data {
        for (k, v) in data_obj {
            flat.insert(k.clone(), v.clone());
        }
    }
    Value::Object(flat)
}

/// 单次会话调用入口（spawned task）
pub struct OrchestratorDeps {
    pub pool: MySqlPool,
    pub s3: S3Client,
    pub llm: Arc<LlmRegistry>,
    pub registry: Arc<CapabilityRegistry>,
    pub invoker: Arc<Invoker>,
    pub ext_pool: Option<MySqlPool>,
    pub message: String,
    pub channel: String,
    pub platform: String,
    pub app_version: String,
}

pub async fn run_session_user(
    deps: OrchestratorDeps,
    session_id: i64,
    starting_agent_id: i64,
    actor_id: i64,
    history: Vec<crate::models::ChatMessageUser>,
    user_content: String,
    tx: UnboundedSender<Result<Event, Infallible>>,
) -> Option<ChatMessageUser> {
    run_session_internal_impl(
        deps,
        session_id,
        starting_agent_id,
        actor_id,
        &history,
        &user_content,
        &tx,
    )
    .await
}
// ============================ Core implementation ============================

/// Trait for accessing role + content on chat messages generically
trait HasRoleContent {
    fn role_ref(&self) -> &str;
    fn content_ref(&self) -> Option<&str>;
}
impl HasRoleContent for crate::models::ChatMessageUser {
    fn role_ref(&self) -> &str {
        &self.role
    }
    fn content_ref(&self) -> Option<&str> {
        self.content.as_deref()
    }
}

async fn run_session_internal_impl<T>(
    deps: OrchestratorDeps,
    session_id: i64,
    starting_agent_id: i64,
    actor_id: i64,
    history: &[T],
    user_content: &str,
    tx: &UnboundedSender<Result<Event, Infallible>>,
) -> Option<ChatMessageUser>
where
    T: HasRoleContent,
{
    let elapsed_start = Instant::now();

    // ★ Create AgentContext FIRST — needed by HookDeps for function/workflow hooks
    let agent_ctx = Arc::new(AgentContext::new(
        format!("session-{session_id}-agent-{starting_agent_id}"),
        UserInput {
            raw_text: user_content.to_string(),
            session_id: Some(session_id.to_string()),
            message_id: None,
            timestamp: Utc::now(),
            metadata: {
                let mut m = std::collections::HashMap::new();
                m.insert("channel".into(), deps.channel.clone());
                m.insert("platform".into(), deps.platform.clone());
                m.insert("app_version".into(), deps.app_version.clone());
                m.insert("actor_id".into(), actor_id.to_string());
                m
            },
        },
        ContextConfig::default(),
    ));

    let hook_deps = HookDeps {
        s3: deps.s3.clone(),
        llm: Arc::clone(&deps.llm),
        registry: Arc::clone(&deps.registry),
        invoker: Arc::clone(&deps.invoker),
        ext_pool: deps.ext_pool.clone(),
        agent_ctx: Arc::clone(&agent_ctx),
    };
    let mut current_agent_id = starting_agent_id;
    let mut visited: Vec<i64> = vec![starting_agent_id];
    let max_hops = max_hops();
    let mut final_content: Option<String> = None;
    let mut final_agent_id = starting_agent_id;
    // 保存最后一次 hop 的 hooks/identifier，用于循环结束后触发 after_agent_end hook
    let mut last_hooks: Option<std::collections::HashMap<String, Vec<crate::models::agent_hook::AgentHook>>> = None;
    let mut last_identifier: Option<String> = None;

    // 把 history 转成 LLM-side messages（OpenAI-style），跳过空内容消息
    let mut messages: Vec<Value> = Vec::new();
    let mut last_user_content: Option<String> = None;
    for m in history {
        if let Some(c) = m.content_ref() {
            if c.is_empty() {
                continue;
            }
            messages.push(json!({"role": m.role_ref(), "content": c}));
            if m.role_ref() == "user" {
                last_user_content = Some(c.to_string());
            }
        }
    }
    // Only append user_content if it's not already the last user message in history
    if last_user_content.as_deref() != Some(user_content) {
        messages.push(json!({"role": "user", "content": user_content}));
    }

    for hop in 0..max_hops {
        // 1. 装配当前 agent 资源
        let agent_content = match build_agent_content(&deps.pool, current_agent_id).await {
            Ok(c) => c,
            Err(e) => {
                emit_error(&tx, 5000, format!("agent context: {e}"));
                break;
            }
        };

        // 缓存 hooks/identifier，让循环结束后 finalize_with_variant 可触发 after_agent_end
        last_hooks = Some(agent_content.hooks.clone());
        last_identifier = Some(agent_content.identifier.clone());

        // ★ before_agent_start hook (blocking-capable)
        {
            let hook_context = HookContext {
                agent_id: agent_content.agent_id,
                identifier: agent_content.identifier.clone(),
                session_id,
                actor_id,
                request_id: String::new(),
                trigger_point: "before_agent_start".into(),
                message: deps.message.clone(),
                channel: deps.channel.clone(),
                platform: deps.platform.clone(),
                app_version: deps.app_version.clone(),
            };
            if let Err(e) = hook::run_hooks(
                Arc::new(deps.pool.clone()),
                &agent_content.hooks,
                "before_agent_start",
                &hook_context,
                &hook_deps,
            )
            .await
            {
                emit_error(&tx, 6005, e.to_string());
                break;
            }
        }

        // 2. 构造 provider
        let (provider, model) = match deps.llm.build_primary(agent_content.model_preset.as_deref()) {
            Ok(p) => p,
            Err(e) => {
                emit_error(&tx, 5007, format!("preset error: {e}"));
                break;
            }
        };

        // 3. 准备 system + tools
        let mut hop_msgs: Vec<Value> =
            vec![json!({"role": "system", "content": agent_content.system_prompt})];
        hop_msgs.extend(messages.clone());

        let tools_schema = build_tools_schema(&agent_content);
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

        // ★ before_llm_call hook (blocking-capable)
        {
            let hctx = HookContext {
                agent_id: agent_content.agent_id,
                identifier: agent_content.identifier.clone(),
                session_id,
                actor_id,
                request_id: String::new(),
                trigger_point: "before_llm_call".into(),
                message: deps.message.clone(),
                channel: deps.channel.clone(),
                platform: deps.platform.clone(),
                app_version: deps.app_version.clone(),
            };
            if let Err(e) = hook::run_hooks(
                Arc::new(deps.pool.clone()),
                &agent_content.hooks,
                "before_llm_call",
                &hctx,
                &hook_deps,
            )
            .await
            {
                emit_error(&tx, 6005, e.to_string());
                break;
            }
        }

        let resp = provider
            .chat_stream_with_retry(
                req,
                Some(on_delta),
                None,
                RetryMode::Standard,
                None,
            )
            .await;

        if resp.is_error() {
            let msg = resp
                .content
                .clone()
                .or(resp.error_kind.clone())
                .unwrap_or_else(|| "LLM error".into());
            emit_error(&tx, resp.error_status_code.unwrap_or(5000) as u16, msg);
            audit_llm(&deps.pool, current_agent_id, "error", elapsed_start, &model).await;
            // ★ on_agent_error hook (audit-only)
            {
                let hctx = HookContext {
                    agent_id: agent_content.agent_id,
                    identifier: agent_content.identifier.clone(),
                    session_id,
                    actor_id,
                    request_id: String::new(),
                    trigger_point: "on_agent_error".into(),
                    message: deps.message.clone(),
                    channel: deps.channel.clone(),
                    platform: deps.platform.clone(),
                    app_version: deps.app_version.clone(),
                };
                let _ = hook::run_hooks(
                    Arc::new(deps.pool.clone()),
                    &agent_content.hooks,
                    "on_agent_error",
                    &hctx,
                    &hook_deps,
                )
                .await;
            }
            // ★ AgentContext: LLM error → terminate
            let _ = agent_ctx.set_lifecycle_state(LifecycleState::Terminated);
            break;
        }

        // ★ after_llm_call hook (audit-only)
        {
            let hctx = HookContext {
                agent_id: agent_content.agent_id,
                identifier: agent_content.identifier.clone(),
                session_id,
                actor_id,
                request_id: String::new(),
                trigger_point: "after_llm_call".into(),
                message: deps.message.clone(),
                channel: deps.channel.clone(),
                platform: deps.platform.clone(),
                app_version: deps.app_version.clone(),
            };
            let _ = hook::run_hooks(
                Arc::new(deps.pool.clone()),
                &agent_content.hooks,
                "after_llm_call",
                &hctx,
                &hook_deps,
            )
            .await;
        }

        // assistant content + tool_calls 都加入 messages，下一轮接 tool messages
        let assistant_content = resp.content.clone().unwrap_or_default();
        let tool_calls = resp.tool_calls.clone();

        // 把 assistant 消息加入 history（含 tool_calls 序列化）
        if !tool_calls.is_empty() {
            let tc_json: Vec<Value> = tool_calls
                .iter()
                .map(|tc| tc.to_openai_tool_call())
                .collect();
            messages.push(json!({
                "role": "assistant",
                "content": if assistant_content.is_empty() { Value::Null } else { Value::String(assistant_content.clone()) },
                "tool_calls": tc_json,
            }));
        } else {
            messages.push(json!({"role": "assistant", "content": assistant_content.clone()}));
        }

        audit_llm(
            &deps.pool,
            current_agent_id,
            "success",
            elapsed_start,
            &model,
        )
        .await;

        // 5. 无 tool_call → 这是最终回复
        if !resp.should_execute_tools() {
            final_content = Some(assistant_content);
            final_agent_id = current_agent_id;
            break;
        }

        // 6. 处理 tool_calls
        let mut routed_to: Option<i64> = None;

        for tc in &tool_calls {
            // ★ before_tool_call hook (blocking-capable)
            {
                let hctx = HookContext {
                    agent_id: agent_content.agent_id,
                    identifier: agent_content.identifier.clone(),
                    session_id,
                    actor_id,
                    request_id: String::new(),
                    trigger_point: "before_tool_call".into(),
                    message: deps.message.clone(),
                    channel: deps.channel.clone(),
                    platform: deps.platform.clone(),
                    app_version: deps.app_version.clone(),
                };
                if let Err(e) = hook::run_hooks(
                    Arc::new(deps.pool.clone()),
                    &agent_content.hooks,
                    "before_tool_call",
                    &hctx,
                    &hook_deps,
                )
                .await
                {
                    emit_error(&tx, 6005, e.to_string());
                    // ★ AgentContext: hook abort → terminate
                    let _ = agent_ctx.set_response_payload(
                        ResponsePayload::new("Hook blocked tool execution".to_string()),
                    );
                    let _ = agent_ctx.set_lifecycle_state(LifecycleState::Terminated);
                    // On hook abort, finalize without processing this tool
                    return finalize_with_variant(
                        &deps.pool,
                        session_id,
                        actor_id,
                        &tx,
                        elapsed_start,
                        Some("Hook blocked tool execution".into()),
                        current_agent_id,
                        Some(&agent_content.hooks),
                        Some(&agent_content.identifier),
                        deps.message.clone(),
                        deps.channel.clone(),
                        deps.platform.clone(),
                        deps.app_version.clone(),
                        &hook_deps,
                    )
                    .await;
                }
            }

            // emit tool_call event
            let tc_payload = json!({
                "tool_call_id": tc.id,
                "name": tc.name,
                "args": tc.arguments,
            });
            let _ = tx.send(Ok(Event::default()
                .event("tool_call")
                .data(tc_payload.to_string())));

            let result = if tc.name == ROUTE_TOOL_NAME {
                handle_route_tool(&deps.pool, &agent_content, &visited, tc).await
            } else if let Some(tool_ref) = agent_content.tools.iter().find(|t| t.identifier == tc.name) {
                handle_workspace_tool(&deps, &agent_content, tool_ref, tc, session_id, Arc::clone(&agent_ctx)).await
            } else {
                ToolOutcome::error(format!("未知工具：{}", tc.name))
            };

            // emit tool_result event
            let tr_payload = json!({
                "tool_call_id": tc.id,
                "result": result.payload,
            });
            let _ = tx.send(Ok(Event::default()
                .event("tool_result")
                .data(tr_payload.to_string())));

            // ★ after_tool_call hook (audit-only)
            {
                let hctx = HookContext {
                    agent_id: agent_content.agent_id,
                    identifier: agent_content.identifier.clone(),
                    session_id,
                    actor_id,
                    request_id: String::new(),
                    trigger_point: "after_tool_call".into(),
                    message: deps.message.clone(),
                    channel: deps.channel.clone(),
                    platform: deps.platform.clone(),
                    app_version: deps.app_version.clone(),
                };
                let _ = hook::run_hooks(
                    Arc::new(deps.pool.clone()),
                    &agent_content.hooks,
                    "after_tool_call",
                    &hctx,
                    &hook_deps,
                )
                .await;
            }

            // ★ AgentContext: record tool execution (audit + category store)
            {
                let tool_status = if result.payload.get("error").is_some() {
                    ToolCallStatus::Failure
                } else {
                    ToolCallStatus::Success
                };
                let now = Utc::now();
                let _ = agent_ctx.record_tool_call(
                    tc.name.clone(),
                    serde_json::Value::Object(tc.arguments.clone()),
                    Some(result.payload.clone()),
                    tool_status,
                    now,
                    Some(now),
                );
                let _ = agent_ctx.set_record(
                    Category::ToolResults,
                    format!("{}-{}", tc.name, tc.id),
                    result.payload.clone(),
                    tc.name.clone(),
                    hop,
                );
            }

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
                    // ★ AgentContext: route loop detected → terminate
                    let _ = agent_ctx.set_response_payload(
                        ResponsePayload::new(assistant_content.clone()),
                    );
                    let _ = agent_ctx.set_lifecycle_state(LifecycleState::Terminated);
                    return finalize_with_variant(
                        &deps.pool,
                        session_id,
                        actor_id,
                        &tx,
                        elapsed_start,
                        final_content,
                        final_agent_id,
                        Some(&agent_content.hooks),
                        Some(&agent_content.identifier),
                        deps.message.clone(),
                        deps.channel.clone(),
                        deps.platform.clone(),
                        deps.app_version.clone(),
                        &hook_deps,
                    )
                    .await;
                }
                routed_to = Some(next_agent);
                visited.push(next_agent);
                // emit routed event
                let routed_payload = json!({
                    "agent_id": next_agent,
                    "agent_identifier": result.route_identifier.clone().unwrap_or_default(),
                });
                let _ = tx.send(Ok(Event::default()
                    .event("routed")
                    .data(routed_payload.to_string())));
                audit_route(&deps.pool, current_agent_id, next_agent).await;

                // ★ AgentContext: record delegation
                {
                    let now = Utc::now();
                    let _ = agent_ctx.record_delegation(
                        next_agent.to_string(),
                        String::new(),
                        json!({"from_agent": current_agent_id, "via": "route_to_subagent"}),
                        None,
                        true,
                        now,
                        None,
                    );
                    // Record state change for routing
                    let _ = agent_ctx.set_record(
                        Category::StateChanges,
                        format!("route-hop-{}", hop),
                        json!({"event": "routed", "from": current_agent_id, "to": next_agent}),
                        "orchestrator".into(),
                        hop,
                    );
                }
            }
        }

        // ★ AgentContext: check if a tool requested agent_loop_break
        if agent_ctx.get_metadata("agent_loop_break").as_deref() == Some("true") {
            final_content = Some(String::new());
            final_agent_id = current_agent_id;
            let _ = agent_ctx.set_record(
                Category::StateChanges,
                format!("hop-{}-loop-break", hop),
                json!({"iteration": hop, "event": "agent_loop_break", "triggered_by": "tool_metadata"}),
                "orchestrator".into(),
                hop,
            );
            let _ = agent_ctx.set_lifecycle_state(LifecycleState::Completed);
            break;
        }

        if let Some(next) = routed_to {
            current_agent_id = next;
            // ★ AgentContext: record hop iteration state before routing continue
            {
                let _ = agent_ctx.set_record(
                    Category::StateChanges,
                    format!("hop-{}-routed", hop),
                    json!({"iteration": hop, "event": "agent_routed", "to_agent": next}),
                    "orchestrator".into(),
                    hop,
                );
            }
            // 路由后下一轮继续；保持 messages 累积让新 agent 看到上下文
            continue;
        }

        // 有 tool_calls 但没路由 → 下一轮 LLM 用 tool result 继续
        if hop + 1 == max_hops {
            emit_error(&tx, 5006, format!("已达最大 hop {max_hops}"));
            final_content = Some(assistant_content);
            final_agent_id = current_agent_id;
            // ★ AgentContext: max hops reached → terminate
            let _ = agent_ctx.set_lifecycle_state(LifecycleState::Terminated);
            break;
        }
    }

    // ★ AgentContext: set final response payload and lifecycle state
    {
        if let Some(ref text) = final_content {
            let _ = agent_ctx.set_response_payload(ResponsePayload::new(text.clone()));
            let _ = agent_ctx.set_lifecycle_state(LifecycleState::Completed);
        } else {
            let _ = agent_ctx.set_lifecycle_state(LifecycleState::Terminated);
        }

    }

    finalize_with_variant(
        &deps.pool,
        session_id,
        actor_id,
        &tx,
        elapsed_start,
        final_content,
        final_agent_id,
        last_hooks.as_ref(),
        last_identifier.as_deref(),
        deps.message.clone(),
        deps.channel.clone(),
        deps.platform.clone(),
        deps.app_version.clone(),
        &hook_deps,
    )
    .await
}
async fn finalize_with_variant(
    pool: &MySqlPool,
    session_id: i64,
    actor_id: i64,
    tx: &UnboundedSender<Result<Event, Infallible>>,
    started: Instant,
    content: Option<String>,
    final_agent_id: i64,
    hooks: Option<&std::collections::HashMap<String, Vec<crate::models::agent_hook::AgentHook>>>,
    agent_identifier: Option<&str>,
    message: String,
    channel: String,
    platform: String,
    app_version: String,
    hook_deps: &HookDeps,
) -> Option<ChatMessageUser> {
    let elapsed = started.elapsed().as_millis() as i32;

    // ★ after_agent_end hook — 必须在收集 extensions 之前执行，
    //    因为 hook 中的 workflow/function 可能写入 extensions
    if let (Some(hooks_map), Some(ident)) = (hooks, agent_identifier) {
        let hctx = HookContext {
            agent_id: final_agent_id,
            identifier: ident.to_string(),
            session_id,
            actor_id,
            request_id: String::new(),
            trigger_point: "after_agent_end".into(),
            message: message.clone(),
            channel: channel.clone(),
            platform: platform.clone(),
            app_version: app_version.clone(),
        };
        let _ = hook::run_hooks(
            Arc::new(pool.clone()),
            hooks_map,
            "after_agent_end",
            &hctx,
            hook_deps,
        )
        .await;
    }

    // Collect extensions from AgentContext for persistence (flattened)
    // 在 after_agent_end hook 之后收集，确保 hook 写入的 extensions 被包含
    let exts = hook_deps.agent_ctx.get_extensions();
    let extensions_for_sse: Option<Value> = if exts.is_empty() {
        None
    } else {
        let arr: Vec<Value> = exts.iter().map(|e| flatten_extension(e)).collect();
        Some(Value::Array(arr))
    };
    let extensions_json = extensions_for_sse.clone();

    let saved = if content.as_deref().map_or(false, str::is_empty) && extensions_json.is_none() {
        None
    } else {
        let text = content.as_deref().unwrap_or("");
        append_assistant_message_user(
            pool,
            session_id,
            actor_id,
            text,
            Some(elapsed),
            extensions_json,
        )
        .await
        .ok()
    };

    let done = json!({
        "elapsed_ms": elapsed,
        "final_agent_id": if final_agent_id != 1 { Some(final_agent_id) } else { None },
    });
    let _ = tx.send(Ok(Event::default().event("done").data(done.to_string())));

    // ★ Emit extensions SSE event after done（确保 hook 写入的被包含）
    if let Some(ref ext_arr) = extensions_for_sse {
        let exts_payload = json!({ "extensions": ext_arr });
        let _ = tx.send(Ok(Event::default()
            .event("extensions")
            .data(exts_payload.to_string())));
    }

    saved
}

fn emit_error(tx: &UnboundedSender<Result<Event, Infallible>>, code: u16, message: String) {
    let _ = tx.send(Ok(Event::default()
        .event("error")
        .data(json!({"code": code, "message": message}).to_string())));
}

// ============================ Agent context ============================

#[derive(Debug, Clone)]
pub(crate) struct AgentContent {
    pub(crate) agent_id: i64,
    pub(crate) identifier: String,
    pub(crate) system_prompt: String,
    pub(crate) model_preset: Option<String>,
    pub(crate) tools: Vec<ToolRef>,
    pub(crate) permissions: Vec<String>,
    pub(crate) children: Vec<ChildAgent>,
    pub(crate) hooks: std::collections::HashMap<String, Vec<crate::models::agent_hook::AgentHook>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolRef {
    pub(crate) id: i64,
    pub(crate) identifier: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) kind: i8, // 1 function-wrap, 2 workflow-wrap
    pub(crate) function_id: Option<i64>,
    /// function 的 identifier（builtin 查找用，与 tool identifier 可能不同）
    pub(crate) function_identifier: Option<String>,
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

async fn build_agent_content(pool: &MySqlPool, agent_id: i64) -> Result<AgentContent, String> {
    let row: Option<(String, String, Option<String>)> =
        sqlx::query_as("SELECT identifier, system_prompt, model_preset FROM agents WHERE id = ?")
            .bind(agent_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("agent fetch: {e}"))?;
    let (identifier, mut system_prompt, model_preset) =
        row.ok_or_else(|| format!("agent id={agent_id} not found"))?;

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
    let tool_rows: Vec<(
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
        Option<String>,
        Option<Value>,
    )> = sqlx::query_as(
        r#"SELECT t.id, t.identifier, t.name, t.description, t.kind,
                      t.function_id, t.workflow_id, t.input_schema,
                      f.plugin_id, f.plugin_export, f.identifier,
                      COALESCE(t.required_capabilities, f.required_capabilities)
               FROM tools t
               JOIN agent_tools at ON at.tool_id = t.id
               LEFT JOIN functions f ON f.id = t.function_id
               WHERE at.agent_id = ?
               UNION
               SELECT t.id, t.identifier, t.name, t.description, t.kind,
                      t.function_id, t.workflow_id, t.input_schema,
                      f.plugin_id, f.plugin_export, f.identifier,
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
    for (
        id,
        identifier,
        name,
        desc,
        kind,
        function_id,
        workflow_id,
        input_schema,
        plugin_id,
        plugin_export,
        function_identifier,
        caps_json,
    ) in tool_rows
    {
        let is_builtin_function = kind == 1 && plugin_id.is_none();
        // 元工具（meta-tool）：kind=1 且 function_id=NULL（如 invoke_function / invoke_workflow）
        let is_meta_tool = kind == 1 && function_id.is_none();
        let required_capabilities: Vec<String> = caps_json
            .and_then(|v| v.as_array().cloned())
            .map(|arr| {
                arr.into_iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        tools.push(ToolRef {
            id,
            identifier,
            name,
            description: desc,
            kind,
            function_id,
            function_identifier,
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
    let children_rows: Vec<(i64, String, Option<String>)> =
        sqlx::query_as("SELECT id, identifier, description FROM agents WHERE parent_agent_id = ?")
            .bind(agent_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    let children: Vec<ChildAgent> = children_rows
        .into_iter()
        .map(|(id, ident, desc)| ChildAgent {
            id,
            identifier: ident,
            description: desc,
        })
        .collect();

    let hooks = crate::services::agent_hook::load_hooks_for_agent(pool, agent_id)
        .await
        .unwrap_or_default();

    Ok(AgentContent {
        agent_id,
        identifier,
        system_prompt,
        model_preset,
        tools,
        permissions: perms.into_iter().map(|(c,)| c).collect(),
        children,
        hooks,
    })
}

/// 组装 LLM-side tools schema（OpenAI function-calling format）
pub(crate) fn build_tools_schema(ctx: &AgentContent) -> Vec<Value> {
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
        Self {
            payload,
            route_to: None,
            route_identifier: None,
        }
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
    parent_ctx: &AgentContent,
    visited: &[i64],
    tc: &ToolCallRequest,
) -> ToolOutcome {
    let target_ident = tc
        .arguments
        .get("agent_identifier")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if target_ident.is_empty() {
        return ToolOutcome::error("agent_identifier 缺失".into());
    }
    // 必须是 parent 的直接子 agent
    let child = parent_ctx
        .children
        .iter()
        .find(|c| c.identifier == target_ident);
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
    ctx: &AgentContent,
    tool_ref: &ToolRef,
    tc: &ToolCallRequest,
    session_id: i64,
    agent_ctx: Arc<AgentContext>,
) -> ToolOutcome {
    match tool_ref.identifier.as_str() {
        "invoke_function" => {
            let func_ident = match tc
                .arguments
                .get("function_identifier")
                .and_then(|v| v.as_str())
            {
                Some(s) => s.to_string(),
                None => {
                    return ToolOutcome::error("invoke_function: function_identifier 缺失".into());
                }
            };
            let mut function_input = match tc.arguments.get("function_input") {
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
                return ToolOutcome::error(format!(
                    "invoke_function: function「{func_ident}」不存在"
                ));
            };

            // Capability check: 如果 function 声明了 required_capabilities，校验 agent 权限
            if let Some(ref caps) = func_caps {
                if let Some(arr) = caps.as_array() {
                    let required: Vec<String> = arr
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    if !required.is_empty() {
                        let agent_perms: std::collections::HashSet<&str> =
                            ctx.permissions.iter().map(|s| s.as_str()).collect();
                        let missing: Vec<&str> = required
                            .iter()
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

            // ★ Inject AgentContext snapshot so function can read runtime state
            inject_agent_context_snapshot(&mut function_input, &agent_ctx);

            match func_kind {
                1 if plugin_id.is_none() => {
                    // builtin function — direct call with AgentContext
                    let Some(builtin) = super::builtins::lookup(&func_ident) else {
                        return ToolOutcome::error(format!(
                            "builtin function「{func_ident}」未找到 handler"
                        ));
                    };
                    let bctx = super::builtins::BuiltinContext {
                        pool: &deps.pool,
                        ext_pool: deps.ext_pool.as_ref(),
                        agent_ctx: Some(Arc::clone(&agent_ctx)),
                    };
                    match (builtin.handler)(function_input, &bctx) {
                        Ok(result) => {
                            // ★ Apply AgentContext updates from function output
                            apply_agent_context_updates(&agent_ctx, &result);
                            ToolOutcome::ok(result)
                        }
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
                    match deps
                        .invoker
                        .invoke(
                            &deps.pool,
                            &deps.s3,
                            Arc::clone(&deps.registry),
                            Arc::clone(&deps.llm),
                            pid,
                            export,
                            input_json,
                            dispatch_ctx,
                        )
                        .await
                    {
                        Ok(out_str) => {
                            let parsed: Value = serde_json::from_str(&out_str)
                                .unwrap_or_else(|_| Value::String(out_str));
                            // ★ Apply AgentContext updates from plugin function output
                            apply_agent_context_updates(&agent_ctx, &parsed);
                            ToolOutcome::ok(parsed)
                        }
                        Err(e) => ToolOutcome::error(format!("plugin invoke failed: {e}")),
                    }
                }
                _ => ToolOutcome::error(format!(
                    "invoke_function: function「{func_ident}」kind={func_kind} 不支持"
                )),
            }
        }
        "invoke_workflow" => {
            let wf_ident = match tc
                .arguments
                .get("workflow_identifier")
                .and_then(|v| v.as_str())
            {
                Some(s) => s.to_string(),
                None => {
                    return ToolOutcome::error("invoke_workflow: workflow_identifier 缺失".into());
                }
            };
            let mut workflow_input = match tc.arguments.get("workflow_input") {
                Some(v) => v.clone(),
                None => return ToolOutcome::error("invoke_workflow: workflow_input 缺失".into()),
            };

            // ★ Inject AgentContext snapshot so workflow can read runtime state
            inject_agent_context_snapshot(&mut workflow_input, &agent_ctx);

            // 查询 workflow id
            let wf_row: Option<(i64,)> =
                sqlx::query_as("SELECT id FROM workflows WHERE identifier = ?")
                    .bind(&wf_ident)
                    .fetch_optional(&deps.pool)
                    .await
                    .map_err(|e| format!("workflow lookup: {e}"))
                    .unwrap_or(None);
            let Some((workflow_id,)) = wf_row else {
                return ToolOutcome::error(format!(
                    "invoke_workflow: workflow「{wf_ident}」不存在"
                ));
            };
            let executor_deps = crate::runtime::workflow::ExecutorDeps {
                pool: deps.pool.clone(),
                s3: deps.s3.clone(),
                registry: Arc::clone(&deps.registry),
                llm: Arc::clone(&deps.llm),
                invoker: Arc::clone(&deps.invoker),
                ext_pool: deps.ext_pool.clone(),
            };
            let executor = crate::runtime::workflow::WorkflowExecutor::new();
            match executor
                .execute(
                    &executor_deps,
                    workflow_id,
                    workflow_input,
                    ctx.agent_id,
                    Arc::clone(&agent_ctx),
                )
                .await
            {
                Ok(outcome) => {
                    // ★ Apply AgentContext updates from workflow output
                    apply_agent_context_updates(&agent_ctx, &outcome.end_value);
                    ToolOutcome::ok(outcome.end_value)
                }
                Err(e) => ToolOutcome::error(format!("workflow execute: {e}")),
            }
        }
        _ => ToolOutcome::error(format!("未知元工具: {}", tool_ref.identifier)),
    }
}

pub(crate) async fn handle_workspace_tool(
    deps: &OrchestratorDeps,
    ctx: &AgentContent,
    tool_ref: &ToolRef,
    tc: &ToolCallRequest,
    session_id: i64,
    agent_ctx: Arc<AgentContext>,
) -> ToolOutcome {
    // Capability check: 如果 tool 声明了 required_capabilities，校验 agent 权限
    if !tool_ref.required_capabilities.is_empty() {
        let agent_perms: std::collections::HashSet<&str> =
            ctx.permissions.iter().map(|s| s.as_str()).collect();
        let missing: Vec<&str> = tool_ref
            .required_capabilities
            .iter()
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
                handle_meta_tool(deps, ctx, tool_ref, tc, session_id, Arc::clone(&agent_ctx)).await
            } else if tool_ref.is_builtin_function {
                if tool_ref.function_id.is_none() {
                    return ToolOutcome::error("builtin function 缺 function_id".into());
                };
                // 使用 function 的 identifier 查找 builtin handler（可能与 tool identifier 不同）
                let lookup_id = tool_ref.function_identifier.as_deref().unwrap_or(&tool_ref.identifier);
                let Some(builtin) = super::builtins::lookup(lookup_id) else {
                    return ToolOutcome::error(format!(
                        "builtin function「{lookup_id}」未找到 handler"
                    ));
                };
                let mut args_value: Value = Value::Object(tc.arguments.clone());
                // ★ Inject AgentContext snapshot for builtin function in tool path
                inject_agent_context_snapshot(&mut args_value, &agent_ctx);
                let bctx = super::builtins::BuiltinContext {
                    pool: &deps.pool,
                    ext_pool: deps.ext_pool.as_ref(),
                    agent_ctx: Some(Arc::clone(&agent_ctx)),
                };
                match (builtin.handler)(args_value, &bctx) {
                    Ok(result) => {
                        apply_agent_context_updates(&agent_ctx, &result);
                        ToolOutcome::ok(result)
                    }
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
            let mut args_value: Value = Value::Object(tc.arguments.clone());
            // ★ Inject AgentContext snapshot — 与 invoke_workflow / hook 路径保持一致
            inject_agent_context_snapshot(&mut args_value, &agent_ctx);
            let executor_deps = crate::runtime::workflow::ExecutorDeps {
                pool: deps.pool.clone(),
                s3: deps.s3.clone(),
                registry: Arc::clone(&deps.registry),
                llm: Arc::clone(&deps.llm),
                invoker: Arc::clone(&deps.invoker),
                ext_pool: deps.ext_pool.clone(),
            };
            // 临时构造 executor — 直接用 sentinel；workflows 持有也行
            let executor = crate::runtime::workflow::WorkflowExecutor::new();
            match executor
                .execute(
                    &executor_deps,
                    workflow_id,
                    args_value,
                    ctx.agent_id,
                    Arc::clone(&agent_ctx),
                )
                .await
            {
                Ok(outcome) => {
                    // ★ Apply AgentContext updates — 与 invoke_workflow / hook 路径保持一致
                    apply_agent_context_updates(&agent_ctx, &outcome.end_value);
                    ToolOutcome::ok(outcome.end_value)
                }
                Err(e) => ToolOutcome::error(format!("workflow execute: {e}")),
            }
        }
        _ => ToolOutcome::error(format!("未知 tool kind: {}", tool_ref.kind)),
    }
}

// ============================ Audit ============================

async fn audit_llm(pool: &MySqlPool, agent_id: i64, outcome: &str, started: Instant, model: &str) {
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
