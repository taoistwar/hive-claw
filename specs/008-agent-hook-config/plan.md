# Implementation Plan: Agent Hook 配置管理

**Branch**: `008-agent-hook-config` | **Date**: 2026-06-02 | **Revised**: 2026-07-24 | **Spec**: [specs/008-agent-hook-config/spec.md](./spec.md)
**Input**: Feature specification from `/specs/008-agent-hook-config/spec.md`

## Summary

在 Agent 运行时中引入可配置的 **Hook 系统**，管理员可在管理中心为每个 Agent 绑定生命周期钩子。Hook 在 7 个关键执行节点自动触发，支持 3 种动作类型：调用 Function、调用 Workflow、HTTP Webhook 回调。Function/Workflow 动作读取 `_agent_context` 快照，并可通过受控 `_agent_context_updates` 更新当前执行的 `AgentContext`；该通道不直接修改持久化 Agent 配置、system prompt 或审计策略。Hook 执行结果只输出结构化 tracing，不持久化执行历史。Hook 默认异步非阻塞，可选阻塞模式；Webhook 仅对三类 eligible transient failure 异步重试（最多 3 次，固定 1/2/4 秒退避）。Assistant 请求边界显式创建带 request/session 关联 ID 的 `RuntimeExecutionContext`；进入 Hook 后审计模式单向降级为 tracing-only，并沿 Hook → Workflow → Plugin → Capability 整条子链保持 sticky。

**复用策略**：Hook 运行时直接在 orchestrator 中内联（方案 A），轻量且与 hiveweb 深度耦合，复用现有 `CapabilityRegistry`、`Invoker`、`WorkflowExecutor`、SSRF 防护、JWT 鉴权和 tracing 基础设施。

## Technical Context

**Language/Version**：Rust 1.97.1（后端，精确锁定），TypeScript 5.x（前端）
**Primary Dependencies**：
- 后端：`axum`、`sqlx` (MySQL)、`tokio`、`serde_json`、`tower-http`
- LLM 客户端：复用 `crates/providers`（`LLMProvider` trait）
- 前端：React + Ant Design + axios
**Storage**：MySQL（仅 Hook 配置）；Hook 执行结果仅进入结构化日志管道；复用 Rustfs/S3（无需新增存储）
**Testing**：cargo test（集成测试）；Vitest + Testing Library；Playwright E2E
**Target Platform**：Linux server
**Project Type**：Web application（hiveweb crate 扩展 + web-admin 前端扩展）
**Performance Goals**：
- Hook 调度开销 p95 ≤ 50ms（SC-003）
- 任意 Hook 执行及 Webhook 重试新增执行历史数据库行数为 0（SC-006）
- Webhook 超时可配置 ±1s 偏差（SC-004）
**Constraints**：
- 每触发点最多 5 个 Hook（FR-001）
- 单次 Hook 超时默认 10 秒（FR-011，可配 `HOOK_TIMEOUT_MS`）
- Hook 失败 100% 产生结构化 tracing（SC-005）
- Hook 子链的结构化审计 tracing 保留父级 request/session 关联 ID，且不得重新启用 runtime audit DB 入队（FR-021–FR-023）
- Hook 动作只能用 `_agent_context_updates` 的固定 shape 更新本次内存 `AgentContext`；业务副作用仍受被选 Function/Workflow 的既有合同约束，Plugin/Capability 路径继续执行 Agent permission 与 Capability 鉴权（FR-024–FR-026）
- 复用现有 RBAC：main Agent Hook = Super only（FR-017）
**Scale/Scope**：预计 500 个 Agent，每 Agent 最多 35 个 Hook（7 触发点 × 5），总配置量 ≤ 17,500

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

✅ **Principle I - Code Quality & Maintainability**：cargo fmt / clippy `-D warnings` / ESLint / Prettier 沿用现有 CI。新增文件遵循现有模块命名约定（`models/agent_hook.rs`、`services/agent_hook.rs`、`api/agent_hook.rs`、`runtime/hook.rs`）。

✅ **Principle II - Test-First Development (NON-NEGOTIABLE)**：Phase 2 将产生红灯测试清单。覆盖：Hook CRUD 契约测试、Hook 执行契约测试（7 触发点 + 3 动作类型组合）、Webhook 重试集成测试、阻塞/非阻塞模式集成测试、权限边界测试。

✅ **Principle III - User Experience Consistency**：Hook 配置 UI 集成到现有 Agent 编辑页面；错误码沿用 `AppError` 枚举 + `codes::*` 模式；不展示无法由持久化数据支撑的执行历史页面。

✅ **Principle IV - Performance & Efficiency**：Hook 调度在 orchestrator 中内联（无额外 IPC/序列化开销），串行执行避免并发锁竞争。Webhook 重试走后台 `tokio::spawn`，不阻塞主流程；次数严格为 0..=3，退避固定 1/2/4 秒。每个 attempt 使用一个覆盖 DNS→client→send 的 Tokio timeout，避免 `run_hooks` 同 deadline 竞态。每个 hop 通过 `fetch_content` 获取 `AgentContent.hooks` 并固定当前快照；Hook CRUD 成功后 best-effort 失效既有 AgentContent 缓存，使下一 hop 可获取新配置。终止错误的 `on_agent_error` 阶段另有固定 30 秒总预算，防止多个 Hook 的独立超时累积为无界错误响应延迟。

✅ **Principle V - Simplicity & YAGNI**：Hook 运行时直接内联到 orchestrator（方案 A），不引入抽象层或消息队列。不实现 Hook 条件触发（按 spec scope），也不引入独立的 Hook-to-Hook 消息协议；后续步骤若需读取前序动作结果，只复用受控 `AgentContext` updates。Webhook 重试用简单的 tokio 异步循环，不引入独立 job queue。

✅ **Principle VI - Observability & Structured Logging**：每次 Hook 执行和 Webhook 重试输出结构化 tracing；错误脱敏且不记录完整消息、Webhook URL 或 payload。普通 Assistant runtime audit 使用携带 request/session 关联 ID 的显式上下文，并可有界 best-effort 入库；进入 Hook 后保留相同关联 ID 且永久降级为 tracing-only，Workflow、Plugin、Capability 子层不得恢复 DB audit。

✅ **Security Requirements**：
- Hook 配置权限沿用现有 RBAC（Super / System+）。
- Hook 执行的 capability 鉴权沿用当前 Agent 的 permissions（FR-013）。
- HTTP Webhook 与 `network.http` 复用同一出站 URL/DNS/transport helper：URL
  解析、HTTPS-only、metadata/非公网 IPv4/IPv6 拒绝、完整 DNS 答案校验、
  单请求 DNS pinning、禁环境代理和禁重定向。创建、有效动作更新、执行及每次
  重试都走该 helper；仅更新 `action_params` 不能绕过。helper 返回 typed
  `Policy` / `ResolveUnavailable`。Hook 仅重试 `ResolveUnavailable`、reqwest
  connection error（`is_connect()`）与 attempt timeout；Policy/header/client
  build/其他 Request/HTTP non-2xx 均不重试。
- Webhook payload 不携带敏感 header（Authorization 等由管理员显式配置）。
- `chat_respond` 内置 Function 禁止被 Hook 调用。
- Function/Workflow 只能通过 `_agent_context_updates` 的固定结构申请当前内存上下文更新；其他输出字段不得改变 `AgentContext`，且该通道不提供持久化配置或数据库写入旁路。

✅ **Technology Stack**：Rust + axum + MySQL + React 符合宪法 v1.4.0。不新增外部依赖。

**Gate Result**：PASS — 无宪法偏离。Hook 系统是对现有 orchestrator 的增量扩展，不引入新组件或模式。

## Project Structure

### Documentation (this feature)

```text
specs/008-agent-hook-config/
├── plan.md              # 本文件
├── research.md          # Phase 0：技术决策
├── data-model.md        # Phase 1：实体 + DDL
├── quickstart.md        # Phase 1：开发上手
├── contracts/
│   └── api.md           # HTTP REST 契约
├── checklists/
│   └── requirements.md  # Spec 质量检查清单
└── tasks.md             # /speckit-tasks 产出
```

### Source Code (repository root)

```text
crates/hiveweb/
├── migrations/
│   ├── V022__create_agent_hooks_table.sql
│   └── V023__create_hook_executions_table.sql # 仅为已发布迁移兼容保留，运行时不再使用
├── src/
│   ├── models/
│   │   └── agent_hook.rs          # AgentHook 配置模型
│   ├── services/
│   │   └── agent_hook.rs          # Hook CRUD service
│   ├── api/
│   │   └── agent_hook.rs          # Hook REST API handlers
│   ├── runtime/
│   │   ├── execution_context.rs   # 显式关联 ID + sticky audit persistence mode
│   │   └── hook.rs                # Hook 执行引擎 + Webhook 重试
│   ├── utils/
│   │   └── error.rs               # 新增 Hook 错误码（6001-6006）
│   └── api/
│       └── mod.rs                 # 注册 Hook 路由

web-admin/
├── src/
│   ├── components/
│   │   └── AgentHookEditor/       # Hook 配置面板组件
│   │       ├── AgentHookEditor.tsx
│   │       ├── HookFormModal.tsx   # 新建/编辑 Hook 表单
│   ├── pages/
│   │   └── AgentEdit.tsx          # Agent 编辑页（集成 Hook Tab）
│   └── services/
│       └── agentHook.ts           # Hook API 客户端
```

**Structure Decision**：后端按现有三层模式（models/services/api/runtime）增量扩展；前端 Hook 编辑面板作为 Agent 编辑页的子组件，与现有 Tool/Skill 多选面板模式一致。

## Hook Execution Flow

**设计原则**: Hook 是受控运行时扩展点。Function/Workflow 获得当前 `AgentContext` 的只读 `_agent_context` 快照；动作结果只有顶层 `_agent_context_updates` 会通过 allowlisted `AgentContext` API 应用到本次执行。其他输出不修改上下文，且更新通道不直接写持久化 Agent 配置、system prompt 或 runtime audit DB。执行审计仍只输出 tracing。

### Orchestrator 集成点

Hook 执行引擎在 `runtime/hook.rs` 中实现，通过 `run_hooks()` 函数在 orchestrator 中调用。以下是具体的插入位置和错误处理策略（对照 `runtime/orchestrator.rs` 现有代码）：

```
chat_assistant {
    execution_context =
        RuntimeExecutionContext::best_effort(Some(request_id), Some(session_id));
    run_session(OrchestratorDeps { execution_context, ... });
}

run_session_internal_impl() {
    // ...[setup]...
    // 首次在 Hook 边界派生；保留 request_id/session_id，
    // 将 audit persistence 单向降级为 tracing-only；
    // 下游可重复 for_hook()，但仍不能升级
    hook_deps = HookDeps {
        execution_context: deps.execution_context.for_hook(),
        ...
    };

    for hop in 0..max_hops {
        agent_content = fetch_content(current_agent_id);
        // 成功后固定当前 hop 快照，并替换 last_loaded_hooks；
        // 下一 hop 重新 fetch，失败则保留最近一次成功快照
        loaded_hooks = LoadedAgentHooks::from(agent_content);

        // ★ before_agent_start hook
        run_hooks(&agent_content.hooks, "before_agent_start", ctx).await;

        build_primary();    → model_preset 解析

        // [system_prompt + tools_schema 组装]...

        // ★★ before_llm_call hook（per-hop）
        run_hooks(&agent_content.hooks, "before_llm_call", ctx).await;

        chat_stream_with_retry();  → LLM 调用

        // ★★ after_llm_call hook（per-hop）
        run_hooks(&agent_content.hooks, "after_llm_call", ctx).await;

        for each tool_call {
            // ★★ before_tool_call hook
            run_hooks(&agent_content.hooks, "before_tool_call", ctx).await;

            handle_route_tool() / handle_workspace_tool();

            // ★★ after_tool_call hook
            run_hooks(&agent_content.hooks, "after_tool_call", ctx).await;
        }
    }

    finalize_with_variant() {
        // ★ after_agent_end hook（仅成功路径；在扩展收集和消息持久化之前）
        run_hooks(&last_loaded_hooks.hooks, "after_agent_end", ctx).await;

        append_assistant_message();   → 持久化 final 消息
        return saved message;
    }
}

// 所有终止错误进入单一 terminate_agent_error：
// 1. 将 AgentContext / ModelPreset / Provider / BlockingHook /
//    RouteLoop / MaxHops 映射为强类型原始错误
// 2. Hook 快照已加载 → 在固定 30 秒总预算内 tracing-only 执行
//    on_agent_error；其失败或预算耗尽仅记录白名单 tracing，
//    不覆盖原错误且不递归
// 3. 首次 fetch_content 失败、快照尚未加载 → 明确跳过；
//    后续 fetch_content 失败 → 使用 last_loaded_hooks
// 4. 将未修改的原始错误发送到内部事件和强类型错误通道
//
// 阻塞 Hook 额外保持：
// - Timeout → 6004；BlockingFailed → 6005
// - 直接返回，不进入 finalize_with_variant，不保存 assistant 占位消息
// 非阻塞 Hook 及既有非 Hook 终止分支保持各自原有 finalize 语义。
//
// chat_assistant 通过独立强类型错误通道接收终止错误，配额恰好回滚一次，
// 然后由 POST /api/assistant 返回同步 JSON；不解析内部 Event，也不提供 admin SSE。
```

### Hook 执行引擎 (`runtime/hook.rs`)

```text
pub async fn run_hooks(
    hooks: &[(trigger_point, Vec<AgentHook>)],
    point: &str,
    ctx: &HookContext,
    pool: Arc<MySqlPool>,       // 仅供 Function/Workflow 动作执行，不用于记录 Hook 历史
    deps: &HookDeps,            // 内含 sticky tracing-only execution_context
) -> Result<(), HookError> {
    let list = get_hooks_for_point(hooks, point);  // 获取该触发点的 Hook，已按 seq 排序
    for hook in list {
        if !hook.enabled { continue; }

        let result = tokio::time::timeout(
            Duration::from_millis(hook.timeout_ms.unwrap_or(10_000)),
            execute_hook_action(hook, ctx, deps),
        ).await;

        match result {
            Ok(Ok(_)) => trace_hook_execution(hook, "success"),
            Ok(Err(e)) => {
                trace_hook_execution(hook, "error", bounded_error_kind(hook.action_type));
                if hook.blocking_mode {
                    return Err(e);  // 阻塞模式 → 中止
                }
                // 非阻塞模式 → 继续后续 Hook
            }
            Err(_) => {
                trace_hook_execution(hook, "timeout");
                if hook.blocking_mode {
                    return Err(HookError::Timeout);
                }
            }
        }
    }
    Ok(())
}

// Function/Workflow 输入只读取 _agent_context snapshot；
// 其输出若包含 _agent_context_updates，则通过受控 helper 应用到
// 当前内存 AgentContext。其他字段不修改上下文。
async fn execute_hook_action(hook, ctx, deps) -> Result {
    match hook.action_type {
        "call_function"  => {
            output = invoker.invoke_function(
                function_id,
                hook.args,
                DispatchCtx { execution_context: deps.execution_context.for_hook(), ... },
            );
            apply_agent_context_updates(deps.agent_ctx, output);
        },
        "call_workflow"  => workflow_executor.execute(
            workflow_id,
            hook.args,
            ExecutorDeps { execution_context: deps.execution_context.for_hook(), ... },
            // Workflow node output updates are applied during execution;
            // the standard no-output-schema end path strips the internal field.
        ),
        "http_webhook"   => {
            // 唯一 timeout 包住 DNS + pin client + send；run_hooks 不再外包一层
            let result = timeout(
                hook.timeout_ms,
                send_webhook_attempt(hook, &payload),
            ).await;
            match classify_webhook_result(result) {
                Success => Ok(()),
                Retryable(_kind) => {
                    // 返回 timeout/error 前同步调度；任务不持有数据库连接
                    schedule_retry(hook.clone(), payload, validated_retry_max());
                    Err(safe_hook_error)
                }
                Terminal(_kind) => Err(safe_hook_error),
            }
        }
    }
}

async fn retry_webhook(hook, payload, retry_max: usize) {
    for (attempt, delay) in [1, 2, 4].into_iter().take(retry_max).enumerate() {
        tokio::time::sleep(Duration::from_secs(delay)).await;
        let attempt_number = attempt + 1;
        let result = timeout(
            hook.timeout_ms,
            send_webhook_attempt(&hook, &payload),
        ).await;
        match classify_webhook_result(result) {
            Success => {
                trace_webhook_retry(hook, attempt_number, "success");
                return;
            }
            Terminal(kind) => {
                trace_webhook_retry(hook, attempt_number, kind);
                return; // terminal failure 永不继续重试
            }
            Retryable(kind) if attempt_number == retry_max => {
                trace_webhook_retry(hook, attempt_number, "error");
                return;
            }
            Retryable(kind) => trace_webhook_retry(hook, attempt_number, kind),
        }
    }
}

// Retryable 封闭集合：
// - typed ResolveUnavailable
// - reqwest Request error 且 error.is_connect()
// - attempt-local timeout
// Policy/Header/ClientBuild/其他 Request/HTTP non-2xx 一律 Terminal。
// 分类只读 typed variant/flag，不检查错误字符串或按 HTTP 状态码范围猜测。
```

### 受控 `AgentContext` 更新边界

```text
current AgentContext
  └─ snapshot → action input["_agent_context"]       (read-only value)
       └─ Function / Workflow output
            └─ output["_agent_context_updates"] only
                 ├─ records[]    → AgentContext::set_record
                 ├─ extensions[] → AgentContext::add_extension
                 └─ metadata{}   → AgentContext::set_metadata (string values)
```

- `_agent_context` 包含当前 user input、messages、records 与 extensions 的序列化快照；它不把 `AgentContext` 的可变引用交给 Plugin/Workflow payload。
- Function/Workflow 调用输入先读取 `action_params.args` 对象，两类动作都由
  运行时叠加可信 HookContext 顶层字段，最后注入 `_agent_context`；固定优先级
  为 `args < runtime HookContext < _agent_context`，配置中的同名字段必定被
  可信值覆盖。
- `records` 仅接受运行时已有的 `AgentContext` categories；未知 category 跳过。`source` 与 `iteration` 使用安全默认值。
- `extensions` 通过已有 `ExtensionContent` 类型构造，`metadata` 只应用字符串值。无法应用的单项只输出固定 `error_kind`，不记录原始 payload。
- 没有 `_agent_context_updates` 时不发生上下文修改；普通返回字段不会被隐式解释为状态变更。
- `_agent_context_updates` 是内部保留输出键：更新应用后所有 Workflow 公共 end-output 路径都剥离该键；显式 output schema 声明它时使用既有 5005/HTTP 422 与固定安全消息拒绝。
- 更新只存在于当前 Agent 执行的内存 `AgentContext`，供后续 Hook、Workflow 节点或 orchestrator 步骤读取。它不直接改写持久化 Agent 配置、system prompt 或 runtime audit persistence mode。
- Function/Workflow 自身允许的业务副作用仍遵守被选动作的既有合同；经过 Plugin/Capability 的路径必须使用当前 Agent permissions 与 Capability Dispatcher。`_agent_context_updates` 不新增数据库或外部系统写入能力。

### 显式执行上下文与审计边界

```text
POST /api/assistant
  RuntimeExecutionContext::best_effort(request_id, session_id)
  │  tracing always + bounded best-effort runtime audit DB copy
  └─ Orchestrator
       └─ for_hook()  — preserve correlation, force tracing-only
            └─ Hook action
                 └─ Workflow
                      └─ Plugin Invoker
                           └─ Capability Dispatcher
                                tracing-only; DB audit cannot be re-enabled
```

- 上下文通过函数参数和依赖结构显式传递，不使用 task-local 隐式状态。
- audit persistence mode 是 `RuntimeExecutionContext` 的私有运行时状态；该类型没有 `Default` / `Deserialize`，外部 payload 无法选择或升级审计模式。
- `for_hook()` 对已经 tracing-only 的上下文再次调用仍为 tracing-only，因此嵌套 Hook 或 Hook 调用 Workflow/Plugin/Capability 时不会意外恢复 DB audit。
- runtime audit 与 Hook 生命周期 tracing 均沿用父级 `request_id` / `session_id`。Hook 子链不向 runtime audit DB 队列入队，但正常 Assistant 调用树仍保留有界 best-effort DB 副本。
- HTTP middleware 仅接受 canonical non-nil UUID `X-Request-Id`，否则生成新
  UUID，并在调用下游前覆盖请求 HeaderMap；handler 读取、响应回显、
  RuntimeExecutionContext、HookContext 和 tracing 只传播同一规范化小写 UUID，
  不记录或暴露原始 header。
- 此边界仅约束审计事件的持久化副本；Function/Workflow 的正常元数据读取和业务数据库访问不受影响。

## Startup Initialization Order

008 不引入新的初始化步骤。Hook 表在 migration 阶段创建，运行时由每个 orchestrator hop 通过 `fetch_content` 按需加载。启动顺序保持不变：

1. env 加载 → 2. DB pool 建立 → 3. migration 自动执行（V022-V023 在此）→ 4. capability upsert → 5. builtin upsert → 6. ... → 12. HTTP serve

## Phase 0: Research

研究主题（详见 `research.md`）：
1. **Hook 配置与 AgentContent 的加载策略** — 每个 hop 如何通过既有 AgentContent 缓存加载 hooks，并在 Hook CRUD 后保持缓存一致？
2. **Webhook 重试的持久化** — 重试期间的状态是否需要持久化到 DB（防重启丢失）？
3. **Hook 执行的并发模型** — orchestrator 中所有 Hook 串行 vs 可选并行
4. **Hook 超时与 Agent 超时的交互** — Hook 超时是否计入 Agent 整体超时？
5. **Hook 配置的乐观锁** — 与 Agent 乐观锁分离还是共用同一版本号？
6. **Hook 子链的审计模式传播** — 如何保留 request/session 关联 ID 并保证 nested Hook audit 永久 tracing-only？
7. **Hook 动作的运行时状态更新边界** — 如何允许本次 AgentContext 的受控更新而不提供任意状态或数据库写入旁路？

## Phase 1: Design

设计产出（详见对应文件）：
- **data-model.md**：`agent_hooks` 配置表；已发布的执行历史表标记为遗留且不再写入
- **contracts/api.md**：仅保留 Hook CRUD 端点契约
- **quickstart.md**：本地开发环境搭建与 Hook 功能验证步骤

### Migration Plan

| Migration | 内容 |
|-----------|------|
| V022 | 创建 `agent_hooks` 表（id, agent_id FK, name, description, trigger_point, action_type, action_params JSON, enabled, sort_order, blocking_mode, timeout_ms, created_at, updated_at） |
| V023 | 已发布的执行历史表迁移，因 forward-only 兼容保留；新运行时不再读写，既有数据不在本次变更中删除 |

### Error Codes（6001-6006）

| Code | Name | HTTP | User Message |
|------|------|------|-------------|
| 6001 | HOOK_TRIGGER_LIMIT_EXCEEDED | 422 | 该触发点最多配置 5 个 Hook |
| 6002 | HOOK_REFERENCE_INVALID | 422 | Hook 引用的 Function/Workflow 不存在或已删除 |
| 6003 | HOOK_WEBHOOK_URL_INVALID | 400 | Webhook URL 不合法（仅支持 HTTPS 且不允许内网地址） |
| 6004 | HOOK_EXECUTION_TIMEOUT | 408 | 阻塞模式 Hook 执行超时，Agent 流程已中止 |
| 6005 | HOOK_BLOCKING_FAILED | 500 | Hook（阻塞模式）执行失败，Agent 流程已中止 |
| 6006 | HOOK_NOT_FOUND | 404 | Hook 配置不存在 |

## Phase 2: Tasks

由 `/speckit.tasks` 生成。核心任务分组预期：

1. **数据层**：AgentHook 配置 Migration + Model 定义；执行历史迁移仅兼容保留
2. **服务层**：Hook CRUD service + 权限校验
3. **API 层**：仅 Hook CRUD endpoints
4. **运行时**：Hook 执行引擎 + Webhook 重试 + 结构化 tracing
5. **Orchestrator 集成**：7 个触发点插入 `run_hooks()` 调用
6. **前端**：AgentHookEditor 配置组件
7. **测试**：契约测试 + 集成测试 + E2E

## Complexity Tracking

> 无宪法偏离，无需填写。

## Dependencies

| 依赖项 | 状态 |
|--------|------|
| 004-agent-runtime（CapabilityRegistry / Invoker / WorkflowExecutor / RuntimeAudit） | 已有 |
| 003-admin-center（RBAC / JWT / AppError 模式） | 已有 |
| crates/providers（HTTP client for webhook） | 已有（通过 reqwest 间接依赖） |
| web-admin AgentEditor 页面 | 已有，需集成 Hook Tab |
