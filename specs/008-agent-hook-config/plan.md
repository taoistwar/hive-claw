# Implementation Plan: Agent Hook 配置管理

**Branch**: `008-agent-hook-config` | **Date**: 2026-06-02 | **Spec**: [specs/008-agent-hook-config/spec.md](./spec.md)
**Input**: Feature specification from `/specs/008-agent-hook-config/spec.md`

## Summary

在 Agent 运行时中引入可配置的 **Hook 系统**，管理员可在管理中心为每个 Agent 绑定生命周期钩子。Hook 在 7 个关键执行节点自动触发，支持 3 种动作类型：调用 Function、调用 Workflow、HTTP Webhook 回调。**Hook 为只读观察者**：执行结果仅写入审计日志，不修改 Agent 运行时状态（如 system_prompt）。Hook 默认异步非阻塞，可选阻塞模式；Webhook 失败后异步重试（最多 3 次，指数退避）。每次执行写入审计日志，支持历史查询。

**复用策略**：Hook 运行时直接在 orchestrator 中内联（方案 A），轻量且与 hiveweb 深度耦合，复用现有 `CapabilityRegistry`、`Invoker`、`WorkflowExecutor`、SSRF 防护、JWT 鉴权、审计日志基础设施。

## Technical Context

**Language/Version**：Rust 1.85+（后端），TypeScript 5.x（前端）
**Primary Dependencies**：
- 后端：`axum`、`sqlx` (MySQL)、`tokio`、`serde_json`、`tower-http`
- LLM 客户端：复用 `crates/providers`（`LLMProvider` trait）
- 前端：React + Ant Design + axios
**Storage**：MySQL（Hook 配置 + 执行历史）；复用 Rustfs/S3（无需新增存储）
**Testing**：cargo test（集成测试）；Vitest + Testing Library；Playwright E2E
**Target Platform**：Linux server
**Project Type**：Web application（hiveweb crate 扩展 + web-admin 前端扩展）
**Performance Goals**：
- Hook 调度开销 p95 ≤ 50ms（SC-003）
- Hook 执行历史查询 p95 ≤ 2s（SC-006）
- Webhook 超时可配置 ±1s 偏差（SC-004）
**Constraints**：
- 每触发点最多 5 个 Hook（FR-001）
- 单次 Hook 超时默认 10 秒（FR-011，可配 `HOOK_TIMEOUT_MS`）
- Hook 失败 100% 审计（SC-005）
- 复用现有 RBAC：main Agent Hook = Super only（FR-017）
**Scale/Scope**：预计 500 个 Agent，每 Agent 最多 35 个 Hook（7 触发点 × 5），总配置量 ≤ 17,500

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

✅ **Principle I - Code Quality & Maintainability**：cargo fmt / clippy `-D warnings` / ESLint / Prettier 沿用现有 CI。新增文件遵循现有模块命名约定（`models/agent_hook.rs`、`services/agent_hook.rs`、`api/agent_hook.rs`、`runtime/hook.rs`）。

✅ **Principle II - Test-First Development (NON-NEGOTIABLE)**：Phase 2 将产生红灯测试清单。覆盖：Hook CRUD 契约测试、Hook 执行契约测试（7 触发点 + 3 动作类型组合）、Webhook 重试集成测试、阻塞/非阻塞模式集成测试、权限边界测试。

✅ **Principle III - User Experience Consistency**：Hook 配置 UI 集成到现有 Agent 编辑页面；错误码沿用 `AppError` 枚举 + `codes::*` 模式；Hook 执行历史页面复用现有审计日志 UI 组件。

✅ **Principle IV - Performance & Efficiency**：Hook 调度在 orchestrator 中内联（无额外 IPC/序列化开销），串行执行避免并发锁竞争。Webhook 重试走后台 `tokio::spawn`，不阻塞主流程。Hook 快照在 `build_agent_context` 时一次性加载。

✅ **Principle V - Simplicity & YAGNI**：Hook 运行时直接内联到 orchestrator（方案 A），不引入抽象层或消息队列。不实现 Hook 条件触发（按 spec scope）。不实现 Hook 之间的数据传递。Webhook 重试用简单的 tokio 异步循环，不引入独立 job queue。

✅ **Principle VI - Observability & Structured Logging**：每次 Hook 执行写 `hook_executions` 表；错误脱敏沿用现有 `payload_summary` 脱敏规则；所有记录共享 `request_id` 贯穿链路。

✅ **Security Requirements**：
- Hook 配置权限沿用现有 RBAC（Super / System+）。
- Hook 执行的 capability 鉴权沿用当前 Agent 的 permissions（FR-013）。
- HTTP Webhook URL 校验复用现有 SSRF 防护（内网 IP + 云 metadata 端点拒绝）。
- Webhook payload 不携带敏感 header（Authorization 等由管理员显式配置）。
- `chat.respond` 内置 Function 禁止被 Hook 调用。

✅ **Technology Stack**：Rust + axum + MySQL + React 符合宪法 v1.3.0。不新增外部依赖。

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
│   ├── V049__create_agent_hooks_table.sql
│   └── V050__create_hook_executions_table.sql
├── src/
│   ├── models/
│   │   └── agent_hook.rs          # AgentHook + HookExecution 模型
│   ├── services/
│   │   └── agent_hook.rs          # Hook CRUD service
│   ├── api/
│   │   └── agent_hook.rs          # Hook REST API handlers
│   ├── runtime/
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
│   │       └── HookExecutionLog.tsx # Hook 执行历史列表
│   ├── pages/
│   │   └── AgentEdit.tsx          # Agent 编辑页（集成 Hook Tab）
│   └── services/
│       └── agentHook.ts           # Hook API 客户端
```

**Structure Decision**：后端按现有三层模式（models/services/api/runtime）增量扩展；前端 Hook 编辑面板作为 Agent 编辑页的子组件，与现有 Tool/Skill 多选面板模式一致。

## Hook Execution Flow

**设计原则**: Hook 为只读观察者（read-only observer）。Hook 执行结果**不**注入到 Agent 的 system_prompt 或修改运行时状态，仅写入 `hook_executions` 审计表。

### Orchestrator 集成点

Hook 执行引擎在 `runtime/hook.rs` 中实现，通过 `run_hooks()` 函数在 orchestrator 中调用。以下是具体的插入位置和错误处理策略（对照 `runtime/orchestrator.rs` 现有代码）：

```
run_session_internal_impl() {
    // ...[setup]...
    for hop in 0..max_hops {
        build_agent_context()  → 同时加载 hooks（按 trigger_point 分组 + 按 seq 排序）
        
        // ★ before_agent_start hook
        run_hooks(&ctx.hooks, "before_agent_start", ctx).await;
        
        build_primary();    → model_preset 解析
        
        // [system_prompt + tools_schema 组装]...
        
        // ★★ before_llm_call hook（per-hop）
        run_hooks(&ctx.hooks, "before_llm_call", ctx).await;
        
        chat_stream_with_retry();  → LLM 调用
        
        // ★★ after_llm_call hook（per-hop）
        run_hooks(&ctx.hooks, "after_llm_call", ctx).await;
        
        for each tool_call {
            // ★★ before_tool_call hook
            run_hooks(&ctx.hooks, "before_tool_call", ctx).await;
            
            handle_route_tool() / handle_workspace_tool();
            
            // ★★ after_tool_call hook
            run_hooks(&ctx.hooks, "after_tool_call", ctx).await;
        }
    }
    
    finalize_with_variant() {
        append_assistant_message();   → 持久化 final 消息
        
        // ★ after_agent_end hook（在 done 事件之前）
        run_hooks(&ctx.hooks, "after_agent_end", ctx).await;
        
        emit done event;
    }
}

// ★ on_agent_error hook（在每个 emit_error() 调用之后）
```

### Hook 执行引擎 (`runtime/hook.rs`)

```text
pub async fn run_hooks(
    hooks: &[(trigger_point, Vec<AgentHook>)],
    point: &str,
    ctx: &HookContext,
    pool: Arc<MySqlPool>,       // Arc for 'static lifetime (tokio::spawn compatibility)
    deps: &OrchestratorDeps,
) -> Result<(), HookError> {
    let list = get_hooks_for_point(hooks, point);  // 获取该触发点的 Hook，已按 seq 排序
    for hook in list {
        if !hook.enabled { continue; }
        
        let result = tokio::time::timeout(
            Duration::from_millis(hook.timeout_ms.unwrap_or(10_000)),
            execute_hook_action(hook, ctx, deps),
        ).await;
        
        match result {
            Ok(Ok(_)) => audit_hook_success(&pool, hook).await,
            Ok(Err(e)) => {
                audit_hook_failure(&pool, hook, &e).await;
                if hook.blocking_mode {
                    return Err(e);  // 阻塞模式 → 中止
                }
                // 非阻塞模式 → 继续后续 Hook
            }
            Err(_) => {
                audit_hook_timeout(&pool, hook).await;
                if hook.blocking_mode {
                    return Err(HookError::Timeout);
                }
            }
        }
    }
    Ok(())
}

// 注意：Hook 为只读观察者，execute_hook_action 的返回值仅用于审计，不修改 Agent 状态
async fn execute_hook_action(hook, ctx, deps) -> Result {
    match hook.action_type {
        "call_function"  => invoker.invoke_function(function_id, hook.args, ctx),
        "call_workflow"  => workflow_executor.execute(workflow_id, hook.args),
        "http_webhook"   => {
            match http_post_with_ssrf_check(&hook.webhook_url, &payload).await {
                Ok(_) => Ok(()),
                Err(_) => {
                    // 后台异步重试（不阻塞 Hook 串行流）
                    // pool 通过 Arc<MySqlPool> 传入，满足 'static 生命周期
                    tokio::spawn(retry_webhook(hook.clone(), payload, 3, pool.clone()));
                    Ok(())
                }
            }
        }
    }
}

async fn retry_webhook(hook, payload, remaining: u32, pool: Arc<MySqlPool>) {
    for attempt in 1..=remaining {
        tokio::time::sleep(Duration::from_secs(2u64.pow(attempt - 1))).await;
        match http_post_with_ssrf_check(&hook.webhook_url, &payload).await {
            Ok(_) => { audit_webhook_success(&pool, ...); return; }
            Err(_) if attempt == remaining => { audit_webhook_final_failure(&pool, ...); return; }
            Err(_) => continue,
        }
    }
}
```

## Startup Initialization Order

008 不引入新的初始化步骤。Hook 表在 migration 阶段创建，运行时通过 `build_agent_context` 按需加载。启动顺序保持不变：

1. env 加载 → 2. DB pool 建立 → 3. migration 自动执行（V049-V050 在此）→ 4. capability upsert → 5. builtin upsert → 6. ... → 12. HTTP serve

## Phase 0: Research

研究主题（详见 `research.md`）：
1. **Hook 配置与 Agent Context 的加载策略** — 每次 `build_agent_context` 时 JOIN 加载 hooks 还是独立查询缓存？
2. **Webhook 重试的持久化** — 重试期间的状态是否需要持久化到 DB（防重启丢失）？
3. **Hook 执行的并发模型** — orchestrator 中所有 Hook 串行 vs 可选并行
4. **Hook 超时与 Agent 超时的交互** — Hook 超时是否计入 Agent 整体超时？
5. **Hook 配置的乐观锁** — 与 Agent 乐观锁分离还是共用同一版本号？

## Phase 1: Design

设计产出（详见对应文件）：
- **data-model.md**：`agent_hooks` 表 + `hook_executions` 表 DDL + 索引策略
- **contracts/api.md**：Hook CRUD 端点 + Hook 执行历史查询端点契约
- **quickstart.md**：本地开发环境搭建与 Hook 功能验证步骤

### Migration Plan

| Migration | 内容 |
|-----------|------|
| V049 | 创建 `agent_hooks` 表（id, agent_id FK, name, description, trigger_point, action_type, action_params JSON, enabled, sort_order, blocking_mode, timeout_ms, created_at, updated_at） |
| V050 | 创建 `hook_executions` 表（id, agent_id, hook_id FK SET NULL, session_id, trigger_point, action_type, outcome enum, error_summary TEXT, elapsed_ms, context_snapshot JSON, request_id, created_at） |

### Error Codes（6001-6006）

| Code | Name | HTTP | User Message |
|------|------|------|-------------|
| 6001 | HOOK_TRIGGER_LIMIT_EXCEEDED | 422 | 该触发点最多配置 5 个 Hook |
| 6002 | HOOK_REFERENCE_INVALID | 422 | Hook 引用的 Function/Workflow 不存在或已删除 |
| 6003 | HOOK_WEBHOOK_URL_INVALID | 400 | Webhook URL 不合法（仅支持 HTTPS 且不允许内网地址） |
| 6004 | HOOK_EXECUTION_TIMEOUT | 408 | Hook 执行超时 |
| 6005 | HOOK_BLOCKING_FAILED | 500 | Hook（阻塞模式）执行失败，Agent 流程已中止 |
| 6006 | HOOK_NOT_FOUND | 404 | Hook 配置不存在 |

## Phase 2: Tasks

由 `/speckit.tasks` 生成。核心任务分组预期：

1. **数据层**：Migration (V049-V050) + Model 定义
2. **服务层**：Hook CRUD service + 权限校验
3. **API 层**：Hook CRUD endpoints + Hook 执行历史查询
4. **运行时**：Hook 执行引擎 + Webhook 重试 + 审计集成
5. **Orchestrator 集成**：7 个触发点插入 `run_hooks()` 调用
6. **前端**：AgentHookEditor 组件 + Hook 执行历史页面
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
