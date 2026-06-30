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
use redis::Client as RedisClient;
use serde_json::{Value, json};
use sqlx::MySqlPool;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedSender;

use crate::models::ChatMessageUser;
use crate::runtime::capability::{CapabilityRegistry, DispatchCtx};
use crate::runtime::hook::{
    self, HookContext, HookDeps, apply_agent_context_updates, inject_agent_context_snapshot,
};
use crate::runtime::invoker::Invoker;
use crate::runtime::llm::LlmRegistry;
use crate::services::chat_user::append_assistant_message_user;
use crate::services::runtime_audit::{self, AuditRecord};
use agent::context::{
    AgentContext, Category, ContextConfig, ExtensionContent, LifecycleState, ResponsePayload,
    ToolCallStatus, UserInput,
};

pub const ROUTE_TOOL_NAME: &str = "route_to_subagent";

fn max_hops() -> usize {
    std::env::var("AGENT_MAX_HOPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5)
}

/// Build LLM-side messages from chat history, with content trimming for old messages.
///
/// When history count >= 5, old assistant messages (all but the last one) have their
/// content replaced with a placeholder to reduce context size, while the last assistant
/// message retains its original content.
fn build_chat_messages<T: HasRoleContent>(
    history: &[T],
    user_content: &str,
) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();
    let mut last_user_content: Option<String> = None;

    // Find the index of the last assistant message
    let last_assistant_idx = history
        .iter()
        .enumerate()
        .rfind(|(_, m)| m.role_ref() == "assistant")
        .map(|(i, _)| i);

    let should_trim = history.len() >= 5;

    for (idx, m) in history.iter().enumerate() {
        let c = m.content_ref();
        let ext = m.extensions_ref();
        let has_content = c.is_some_and(|s| !s.trim().is_empty());
        let has_extensions = ext.is_some_and(|v| !v.is_null());

        if !has_content && !has_extensions {
            continue;
        }

        let is_assistant = m.role_ref() == "assistant";
        let is_last_assistant = last_assistant_idx == Some(idx);

        // Old assistant messages: replace entire content with placeholder
        if should_trim && is_assistant && !is_last_assistant {
            messages.push(json!({"role": "assistant", "content": "[已省略]"}));
            continue;
        }

        let msg = if has_extensions {
            let mut obj = serde_json::Map::new();
            obj.insert("content".into(), json!(c.unwrap_or("")));
            obj.insert("extensions".into(), ext.unwrap().clone());
            let content = serde_json::to_string(&Value::Object(obj)).unwrap_or_default();
            json!({"role": m.role_ref(), "content": content})
        } else {
            json!({"role": m.role_ref(), "content": c.unwrap_or("")})
        };
        messages.push(msg);

        if m.role_ref() == "user" {
            if let Some(text) = c {
                last_user_content = Some(text.to_string());
            }
        }
    }

    // Only append user_content if it's not already the last user message in history
    if last_user_content.as_deref() != Some(user_content) {
        messages.push(json!({"role": "user", "content": user_content}));
    }

    messages
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
            tracing::debug!(key = %k, value = %v, "flatten extension data");
            flat.insert(k.clone(), v.clone());
        }
    }
    Value::Object(flat)
}

/// 根据 extensions 中空数据情况重置 content
fn rewrite_content_for_empty_extensions(
    content: Option<String>,
    extensions_for_sse: &Option<Value>,
) -> Option<String> {
    let exts = match extensions_for_sse {
        Some(Value::Array(arr)) => arr,
        _ => return content,
    };

    for ext in exts {
        let ct = ext.get("content_type").and_then(|v| v.as_str());
        if ct != Some("card") {
            continue;
        }

        let payload = ext.get("payload");
        let ptype = payload.and_then(|p| p.get("type")).and_then(|v| v.as_str());

        // support 卡片：根据 category 生成 reply 作为 content
        if ptype == Some("support") {
            let category = payload
                .and_then(|p| p.get("category"))
                .and_then(|v| v.as_str())
                .unwrap_or("other");
            let reply = support_category_reply(category);
            if !reply.is_empty() {
                return Some(reply.to_string());
            }
        }

        let category = ext
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // duration_card 为空
        let dc_empty = payload
            .and_then(|p| p.get("duration_card"))
            .and_then(|v| v.as_array())
            .map(|a| a.is_empty())
            .unwrap_or(true);

        // disk_total_size == 0
        let disk_empty = payload
            .and_then(|p| p.get("info"))
            .and_then(|i| i.get("disk_total_size"))
            .and_then(|v| v.as_f64())
            .map(|s| s == 0.0)
            .unwrap_or(true);

        if category == "duration_card" && dc_empty {
            return Some(
                "暂未查询到您的时长卡购买记录。如您需要更多游戏时长，推荐购买会员产品，通常会比单独购买时长卡更划算。"
                    .into(),
            );
        }

        if category == "disk" && disk_empty {
            return Some(
                "暂未查询到您的云硬盘购买记录。如您需要使用云硬盘，可前往「我的」页面点击「云硬盘」进行购买。您也可以购买会员产品，享受赠送的 5GB 会员专属云硬盘权益。"
                    .into(),
            );
        }
    }

    content
}

/// 根据 support card 的 category 生成对应的回复文案
fn support_category_reply(category: &str) -> &'static str {
    match category {
        "cannot_play" => "抱歉让你遇到无法正常进入游戏的问题。此类情况可能和游戏服务状态、云端环境、网络连接或游戏本身兼容性有关。我们会尽量保障游戏可正常启动，你可以通过下方「联系客服」继续反馈，我们会协助核实处理。",
        "lag" => "抱歉影响了你的游戏体验。云游戏对网络稳定性和当前线路状态比较敏感，网络波动、服务器负载或画质设置都可能导致卡顿、延迟高或掉帧。你可以通过下方「联系客服」反馈，我们会进一步协助排查。",
        "update" => "抱歉当前版本没有及时满足你的使用需求。云游戏内的游戏版本通常需要经过适配、测试和上线流程，可能会比官方版本略有延迟。我们会持续关注版本更新进度，你也可以通过下方「联系客服」反馈具体游戏。",
        "quality" => "抱歉当前画质没有达到你的预期。云游戏画质会受到网络状态、画质设置、设备显示效果以及云端渲染策略影响。我们会持续优化画质体验，你可以通过下方「联系客服」继续反馈问题。",
        "account" => "很抱歉遇到账号异常问题。账号封禁或异常通常由游戏官方规则判断，平台本身无法直接修改游戏官方的处理结果。但如果你怀疑和云游戏登录环境有关，可以通过下方「联系客服」反馈，我们会协助核实。",
        "save_data" => "抱歉给你带来困扰。游戏存档通常和游戏账号、区服、云端同步或游戏自身机制有关，出现丢失时确实会很影响体验。你可以通过下方「联系客服」继续反馈，我们会协助核实是否存在同步异常。",
        "money" => "抱歉影响了你的充值或会员权益。付费后到账可能受到支付状态、服务器端回调或订单同步延迟影响。请先不要重复支付，可以通过下方「联系客服」反馈，我们会优先协助核实订单处理情况。",
        _ => "抱歉这次体验让你不满意，我们理解这种情况会很影响心情。你的反馈对我们很重要，我们会持续优化游戏体验和服务稳定性。你可以通过下方「联系客服」继续反馈，我们会尽力协助处理。",
    }
}

/// 单次会话调用入口（spawned task）
pub struct OrchestratorDeps {
    pub pool: MySqlPool,
    pub redis: RedisClient,
    /// 仅在 `PLUGIN_SYSTEM_ENABLED=true` 时为 `Some`。
    pub s3: Option<S3Client>,
    pub llm: Arc<LlmRegistry>,
    pub registry: Arc<CapabilityRegistry>,
    pub invoker: Arc<Invoker>,
    pub ext_pool: Option<MySqlPool>,
    pub message: String,
    pub channel: String,
    pub client_type: String,
    pub client_version: String,
    /// 010 Sensitive Word Filter — for output content filtering
    pub sensitive_filter: crate::services::sensitive_filter::SensitiveFilter,
    /// 客户端断开时设置为 true，orchestrator 应在安全点检查并退出
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
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
    fn extensions_ref(&self) -> Option<&serde_json::Value>;
}
impl HasRoleContent for crate::models::ChatMessageUser {
    fn role_ref(&self) -> &str {
        &self.role
    }
    fn content_ref(&self) -> Option<&str> {
        self.content.as_deref()
    }
    fn extensions_ref(&self) -> Option<&serde_json::Value> {
        self.extensions.as_ref()
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
                m.insert("client_type".into(), deps.client_type.clone());
                m.insert("client_version".into(), deps.client_version.clone());
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
        redis: Some(deps.redis.clone()),
        agent_ctx: Arc::clone(&agent_ctx),
    };
    let mut current_agent_id = starting_agent_id;
    let mut visited: Vec<i64> = vec![starting_agent_id];
    let max_hops = max_hops();
    let mut final_content: Option<String> = None;
    let mut final_agent_id = starting_agent_id;
    // 保存最后一次 hop 的 hooks/identifier，用于循环结束后触发 after_agent_end hook
    let mut last_hooks: Option<
        std::collections::HashMap<String, Vec<crate::models::agent_hook::AgentHook>>,
    > = None;
    let mut last_identifier: Option<String> = None;

    // 把 history 转成 LLM-side messages（OpenAI-style），跳过空内容消息；
    // 有 extensions 时合并 content + extensions 为一个 JSON 对象，空字段不显示。
    let mut messages = build_chat_messages(&history, user_content);

    // ★ Store conversation history in AgentContext for function/workflow access
    let _ = agent_ctx.set_messages(messages.clone());

    for hop in 0..max_hops {
        // 0. 检查客户端是否已断开
        if deps.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            tracing::info!(
                hop,
                session_id,
                "orchestrator cancelled: client disconnected"
            );
            return None;
        }

        // 1. 装配当前 agent 资源
        let agent_content =
            match crate::services::agent::fetch_content(&deps.pool, &deps.redis, current_agent_id)
                .await
            {
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
                client_type: deps.client_type.clone(),
                client_version: deps.client_version.clone(),
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
        let (provider, model) = match deps
            .llm
            .build_primary(agent_content.model_preset.as_deref())
        {
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
            let hook_context = HookContext {
                agent_id: agent_content.agent_id,
                identifier: agent_content.identifier.clone(),
                session_id,
                actor_id,
                request_id: String::new(),
                trigger_point: "before_llm_call".into(),
                message: deps.message.clone(),
                channel: deps.channel.clone(),
                client_type: deps.client_type.clone(),
                client_version: deps.client_version.clone(),
            };
            if let Err(e) = hook::run_hooks(
                Arc::new(deps.pool.clone()),
                &agent_content.hooks,
                "before_llm_call",
                &hook_context,
                &hook_deps,
            )
            .await
            {
                emit_error(&tx, 6005, e.to_string());
                break;
            }
        }

        let resp = provider
            .chat_stream_with_retry(req, Some(on_delta), None, RetryMode::Standard, None)
            .await;

        if resp.is_error() {
            let msg = resp
                .content
                .clone()
                .or(resp.error_kind.clone())
                .unwrap_or_else(|| "LLM error".into());
            emit_error(&tx, resp.error_status_code.unwrap_or(5000) as u16, msg);
            audit_llm(current_agent_id, "error", elapsed_start, &model).await;
            // ★ on_agent_error hook (audit-only)
            {
                let hook_context = HookContext {
                    agent_id: agent_content.agent_id,
                    identifier: agent_content.identifier.clone(),
                    session_id,
                    actor_id,
                    request_id: String::new(),
                    trigger_point: "on_agent_error".into(),
                    message: deps.message.clone(),
                    channel: deps.channel.clone(),
                    client_type: deps.client_type.clone(),
                    client_version: deps.client_version.clone(),
                };
                let _ = hook::run_hooks(
                    Arc::new(deps.pool.clone()),
                    &agent_content.hooks,
                    "on_agent_error",
                    &hook_context,
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
            let hook_context = HookContext {
                agent_id: agent_content.agent_id,
                identifier: agent_content.identifier.clone(),
                session_id,
                actor_id,
                request_id: String::new(),
                trigger_point: "after_llm_call".into(),
                message: deps.message.clone(),
                channel: deps.channel.clone(),
                client_type: deps.client_type.clone(),
                client_version: deps.client_version.clone(),
            };
            let _ = hook::run_hooks(
                Arc::new(deps.pool.clone()),
                &agent_content.hooks,
                "after_llm_call",
                &hook_context,
                &hook_deps,
            )
            .await;
        }

        // assistant content + tool_calls 都加入 messages，下一轮接 tool messages
        let assistant_content = resp.content.clone().unwrap_or_default();
        let tool_calls = resp.tool_calls.clone();

        // 把 assistant 消息加入 history（含 tool_calls 序列化 + reasoning_content）
        if !tool_calls.is_empty() {
            let tc_json: Vec<Value> = tool_calls
                .iter()
                .map(|tc| tc.to_openai_tool_call())
                .collect();
            let mut msg = json!({
                "role": "assistant",
                "content": if assistant_content.is_empty() { Value::Null } else { Value::String(assistant_content.clone()) },
                "tool_calls": tc_json,
            });
            if let Some(ref rc) = resp.reasoning_content {
                msg["reasoning_content"] = Value::String(rc.clone());
            }
            messages.push(msg);
        } else {
            messages.push(json!({"role": "assistant", "content": assistant_content.clone()}));
        }

        audit_llm(current_agent_id, "success", elapsed_start, &model).await;

        // 5. 无 tool_call → 这是最终回复
        if !resp.should_execute_tools() {
            // Output filter check (010-sensitive-word-filter)
            let filtered_content = filter_output(
                &assistant_content,
                &deps.sensitive_filter,
                &deps.pool,
                session_id,
            )
            .await;
            final_content = Some(filtered_content);
            final_agent_id = current_agent_id;
            break;
        }

        // 6. 处理 tool_calls
        let mut routed_to: Option<i64> = None;

        for tc in &tool_calls {
            // ★ before_tool_call hook (blocking-capable)
            {
                let hook_context = HookContext {
                    agent_id: agent_content.agent_id,
                    identifier: agent_content.identifier.clone(),
                    session_id,
                    actor_id,
                    request_id: String::new(),
                    trigger_point: "before_tool_call".into(),
                    message: deps.message.clone(),
                    channel: deps.channel.clone(),
                    client_type: deps.client_type.clone(),
                    client_version: deps.client_version.clone(),
                };
                if let Err(e) = hook::run_hooks(
                    Arc::new(deps.pool.clone()),
                    &agent_content.hooks,
                    "before_tool_call",
                    &hook_context,
                    &hook_deps,
                )
                .await
                {
                    emit_error(&tx, 6005, e.to_string());
                    // ★ AgentContext: hook abort → terminate
                    let _ = agent_ctx.set_response_payload(ResponsePayload::new(
                        "Hook blocked tool execution".to_string(),
                    ));
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
                        deps.client_type.clone(),
                        deps.client_version.clone(),
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
            } else if let Some(tool_ref) =
                agent_content.tools.iter().find(|t| t.identifier == tc.name)
            {
                handle_workspace_tool(
                    &deps,
                    &agent_content,
                    tool_ref,
                    tc,
                    session_id,
                    Arc::clone(&agent_ctx),
                )
                .await
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
                let hook_context = HookContext {
                    agent_id: agent_content.agent_id,
                    identifier: agent_content.identifier.clone(),
                    session_id,
                    actor_id,
                    request_id: String::new(),
                    trigger_point: "after_tool_call".into(),
                    message: deps.message.clone(),
                    channel: deps.channel.clone(),
                    client_type: deps.client_type.clone(),
                    client_version: deps.client_version.clone(),
                };
                let _ = hook::run_hooks(
                    Arc::new(deps.pool.clone()),
                    &agent_content.hooks,
                    "after_tool_call",
                    &hook_context,
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
                    let _ = agent_ctx
                        .set_response_payload(ResponsePayload::new(assistant_content.clone()));
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
                        deps.client_type.clone(),
                        deps.client_version.clone(),
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
                audit_route(current_agent_id, next_agent).await;

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
        deps.client_type.clone(),
        deps.client_version.clone(),
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
    client_type: String,
    client_version: String,
    hook_deps: &HookDeps,
) -> Option<ChatMessageUser> {
    let elapsed = started.elapsed().as_millis() as i32;

    // ★ after_agent_end hook — 必须在收集 extensions 之前执行，
    //    因为 hook 中的 workflow/function 可能写入 extensions
    if let (Some(hooks_map), Some(ident)) = (hooks, agent_identifier) {
        let hook_context = HookContext {
            agent_id: final_agent_id,
            identifier: ident.to_string(),
            session_id,
            actor_id,
            request_id: String::new(),
            trigger_point: "after_agent_end".into(),
            message: message.clone(),
            channel: channel.clone(),
            client_type: client_type.clone(),
            client_version: client_version.clone(),
        };
        let _ = hook::run_hooks(
            Arc::new(pool.clone()),
            hooks_map,
            "after_agent_end",
            &hook_context,
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

    // ★ 根据 extensions 内容重置 empty content
    let final_content = rewrite_content_for_empty_extensions(content, &extensions_for_sse);

    let saved =
        if final_content.as_deref().map_or(false, str::is_empty) && extensions_json.is_none() {
            None
        } else {
            let text = final_content.as_deref().unwrap_or("");
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
// 类型定义和 DB 读取已迁出到 `services::agent`（fetch_content / AgentContent /
// ToolRef），这里仅 re-export 保留 `pub(crate)` 可见性，让 runtime 内其它模块
// 继续通过 `super::orchestrator::AgentContent` 等路径访问。
pub(crate) use crate::services::agent::{AgentContent, ToolRef};

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
                        redis: Some(&deps.redis),
                        agent_ctx: Some(Arc::clone(&agent_ctx)),
                        llm: Some(&deps.llm),
                        agent_id: Some(ctx.agent_id),
                    };
                    match (builtin.handler)(function_input, &bctx) {
                        Ok(result) => {
                            // ★ Apply AgentContext updates from function output
                            apply_agent_context_updates(&agent_ctx, &result);
                            ToolOutcome::ok(result)
                        }
                        Err(e) => {
                            tracing::error!(
                                func_ident = %func_ident,
                                error = %e,
                                "builtin function 执行失败"
                            );
                            ToolOutcome::error(format!("builtin function 执行失败: {e}"))
                        }
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
                            deps.s3.as_ref(),
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
                        Err(e) => {
                            tracing::error!(
                                func_ident = %func_ident,
                                error = %e,
                                "plugin invoke failed"
                            );
                            ToolOutcome::error(format!("plugin invoke failed: {e}"))
                        }
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
                redis: Some(deps.redis.clone()),
                permissions: ctx.permissions.clone(),
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
                Err(e) => {
                    tracing::error!(
                        workflow_id,
                        wf_ident = %wf_ident,
                        error = %e,
                        "invoke_workflow execution failed"
                    );
                    ToolOutcome::error(format!("workflow execute: {e}"))
                }
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
                let lookup_id = tool_ref
                    .function_identifier
                    .as_deref()
                    .unwrap_or(&tool_ref.identifier);
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
                    redis: Some(&deps.redis),
                    agent_ctx: Some(Arc::clone(&agent_ctx)),
                    llm: Some(&deps.llm),
                    agent_id: Some(ctx.agent_id),
                };
                match (builtin.handler)(args_value, &bctx) {
                    Ok(result) => {
                        apply_agent_context_updates(&agent_ctx, &result);
                        ToolOutcome::ok(result)
                    }
                    Err(e) => {
                        tracing::error!(
                            lookup_id = %lookup_id,
                            error = %e,
                            "builtin function 执行失败"
                        );
                        ToolOutcome::error(format!("builtin function 执行失败: {e}"))
                    }
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
                        deps.s3.as_ref(),
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
                    Err(e) => {
                        tracing::error!(
                            tool = %tool_ref.identifier,
                            error = %e,
                            "plugin invoke failed"
                        );
                        ToolOutcome::error(format!("plugin invoke failed: {e}"))
                    }
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
                redis: Some(deps.redis.clone()),
                permissions: ctx.permissions.clone(),
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
                Err(e) => {
                    tracing::error!(
                        workflow_id,
                        tool = %tool_ref.identifier,
                        error = %e,
                        "handle_workspace_tool workflow execution failed"
                    );
                    ToolOutcome::error(format!("workflow execute: {e}"))
                }
            }
        }
        _ => ToolOutcome::error(format!("未知 tool kind: {}", tool_ref.kind)),
    }
}

// ── Sensitive filter output check ──

/// Check Agent output against the sensitive word filter.
/// If a match is found, replace with a friendly prompt.
async fn filter_output(
    content: &str,
    filter: &crate::services::sensitive_filter::SensitiveFilter,
    _pool: &MySqlPool,
    session_id: i64,
) -> String {
    if let Some(hit) = filter.check(content) {
        tracing::info!(
            session_id,
            triggered_word = %hit.word(),
            "Agent output replaced by sensitive filter"
        );
        return "内容安全警告：输出的文本数据可能包含不适当的内容！".to_string();
    }
    content.to_string()
}

// ============================ Audit ============================

async fn audit_llm(agent_id: i64, outcome: &str, started: Instant, model: &str) {
    let _ = model;
    runtime_audit::record(AuditRecord {
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
    });
}

async fn audit_route(from: i64, to: i64) {
    runtime_audit::record(AuditRecord {
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
    });
}
