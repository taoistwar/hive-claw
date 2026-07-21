---
description: "Task list for Agent Hook Configuration Management"
---

# Tasks: Agent Hook 配置管理

**Input**: Design documents from `/specs/008-agent-hook-config/`
**Prerequisites**: plan.md (required), spec.md (required)

**Tests**: 🔴 测试为强制要求（宪法 Principle II — NON-NEGOTIABLE）。Phase 2.5 红灯测试必须先于对应实现任务编写并提交。

**Organization**: 按 user story 分组，每个 story 可独立交付与验证。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: 可并行（不同文件，无未完成依赖）
- **[Story]**: 所属用户故事（US1..US3）
- 描述中必须含**确切**文件路径

<!--
  ============================================================================
  User Story 来源（spec.md）：
  - US1 (P1) 🎯 MVP — 为 Agent 绑定生命周期钩子（Hook 配置 CRUD + 执行）
  - US2 (P2)      — HTTP Webhook 与外部系统集成
  - US3 (P2)      — Hook 调用 Workflow/Function 实现自动化
  - 2026-07-16 决议 — 执行结果仅输出结构化 tracing，不提供执行历史存储、API 或 UI
  ============================================================================
-->

---

## Phase 1: Setup（共享基础设施）

**Purpose**: 无额外依赖安装，仅需新建文档目录和 env 变量。

- [x] T001 [P] 创建 `specs/008-agent-hook-config/contracts/api.md` 骨架文件（Phase 1 后填充）
- [x] T002 [P] 在 `crates/hiveweb/.env.example` 中新增 Hook 相关 env 变量：`HOOK_TIMEOUT_MS=10000`、`HOOK_WEBHOOK_RETRY_MAX=3`，并标注对应 FR 编号

---

## Phase 2: Foundational（阻塞所有 user story）

**Purpose**: Hook 配置表 + 错误码 + 配置模型定义。所有 user story 实现都依赖此。

**⚠️ CRITICAL**: 此阶段完成前不能动 user story 实现。

### 数据库迁移

- [x] T003 [P] 创建 V022 `agent_hooks` 表迁移 `crates/hiveweb/migrations/V022__create_agent_hooks_table.sql`
  - 列：`id BIGINT AUTO_INCREMENT PRIMARY KEY`、`agent_id BIGINT NOT NULL`、`name VARCHAR(128) NOT NULL`、`description TEXT`、`trigger_point VARCHAR(32) NOT NULL`（7 枚举值 CHECK 约束）、`action_type VARCHAR(32) NOT NULL`（call_function / call_workflow / http_webhook）、`action_params JSON NOT NULL`（含 function_id/workflow_id/webhook_url/headers/timeout_ms/args 等）、`enabled BOOLEAN DEFAULT TRUE`、`sort_order INT DEFAULT 0`、`blocking_mode BOOLEAN DEFAULT FALSE`、`timeout_ms INT DEFAULT 10000`、`created_at DATETIME(6) DEFAULT CURRENT_TIMESTAMP(6)`、`updated_at DATETIME(6) DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6)`
  - 索引：`INDEX idx_agent_hooks_agent (agent_id)`、`UNIQUE INDEX idx_agent_hooks_seq (agent_id, trigger_point, sort_order)`
  - FK：`agent_id → agents(id) ON DELETE CASCADE`
- [x] T004 [P] 历史版本曾创建 V023 `hook_executions` 表迁移 `crates/hiveweb/migrations/V023__create_hook_executions_table.sql`；迁移按 forward-only 原则保留，但当前应用不再读写该表，也不删除既有数据
- [x] T005 在 `crates/hiveweb/src/bin/migrate.rs` 中注册 V022–V023 migration entries

### 错误码 + Model

- [x] T006 [P] 在 `crates/hiveweb/src/utils/error.rs` 新增 Hook 相关错误码常量（6001-6006）和 `AppError` 枚举变体（`HookTriggerLimitExceeded`、`HookReferenceInvalid`、`HookWebhookUrlInvalid`、`HookExecutionTimeout`、`HookBlockingFailed`、`HookNotFound`），含 `code()` / `message()` / `http_status_for_code()` 映射
- [x] T007 [P] 创建 `crates/hiveweb/src/models/agent_hook.rs`：`AgentHook` struct（FromRow + Serialize），含 `CreateHookRequest` / `UpdateHookRequest` / `HookListQuery` 请求 DTO；不定义执行历史模型

**Checkpoint**: Foundation ready — DB 表、错误码、模型就绪，user story 实现可以开始。

---

## Phase 2.5: 红灯测试（Tests BEFORE Implementation）

**Purpose**: 按照 TDD 宪法要求，先写测试 → 观察失败 → 再实现。

> ✅ **TDD 流程**：执行历史移除先由 T056–T058 观察红灯，再修改生产代码；T017 (call_workflow) 仍需系统中配置有效 Workflow，标记 `#[ignore]`。

### US1 红灯测试 — Hook 配置 CRUD + 执行

- [x] T008 [P] [US1] 契约测试：`POST /api/agents/:id/hooks` 创建 Hook → 验证返回 200 + Hook 对象含 id/created_at
- [x] T009 [P] [US1] 契约测试：`GET /api/agents/:id/hooks` 列出 Agent 全部 Hook → 验证返回数组 + 按 sort_order 排序
- [x] T010 [P] [US1] 契约测试：`PUT /api/agents/:id/hooks/:hook_id` 更新 Hook → 验证 optimistic lock (4094) 行为
- [x] T011 [P] [US1] 契约测试：`DELETE /api/agents/:id/hooks/:hook_id` 删除 Hook → 验证返回 200 + 再次 GET 列表不含该 Hook
- [x] T012 [P] [US1] 集成测试：配置 `before_agent_start` Hook（http_webhook）→ 触发 Agent 对话 → 验证 Hook 执行且 `hook_executions` 不新增记录
- [x] T013 [P] [US1] 集成测试：同一触发点配置 6 个 Hook → 验证保存时返回 6001 错误码
- [x] T053 [P] [US1] 集成测试：配置 `before_agent_start` Hook（blocking_mode=true）指向不可达 webhook → 触发 Agent 对话 → 验证 SSE 流发出 error 事件 (SC-007)

### US2 红灯测试 — HTTP Webhook

- [x] T014 [P] [US2] 契约测试：`POST /api/agents/:id/hooks`（action_type=http_webhook, webhook_url=http://...）→ 验证返回 6003（非 HTTPS）
- [x] T015 [P] [US2] 集成测试：配置 Webhook Hook 指向不可达 URL → 触发对话 → 验证失败与重试不会写入 `hook_executions`
- [x] T052 [P] [US2] 契约测试：POST Hook 的 `action_params.headers` 含 `\r\n` header injection → 验证返回 6003（FR-007b）

### US3 红灯测试 — Function/Workflow 调用

- [x] T016 [P] [US3] 契约测试：`POST /api/agents/:id/hooks`（action_type=call_function, function_id 不存在）→ 验证返回 6002
- [ ] T017 [P] [US3] 集成测试：配置 `after_agent_end` Hook（call_workflow）→ 触发对话 → 验证 workflow 被执行 #[ignore] 需 chat 基础设施 + valid Workflow

### 2026-07-16 回归红灯测试 — tracing-only

- [x] T056 [P] 契约测试：`GET /api/agents/:id/hooks/executions` → 验证返回 404/405，且不提供查询结果
- [x] T057 [P] 集成测试：执行 disabled/skipped Hook → 验证 `hook_executions` 新增行数为 0
- [x] T058 [P] 源码守卫测试：Hook 运行时不得包含 `INSERT INTO hook_executions`

---

## Phase 3: User Story 1 — 为 Agent 绑定生命周期钩子 (Priority: P1) 🎯 MVP

**Goal**: 管理员可在 Agent 编辑页配置 Hook，Hook 在 Agent 执行过程中自动触发并输出结构化 tracing。

**Independent Test**: 创建 Agent → 配置 `before_agent_start` Hook → 触发对话 → 验证 Hook 执行 + tracing 字段，并确认数据库无新增执行记录。

### 后端 — Service 层

- [x] T020 [P] [US1] 实现 Hook CRUD service 在 `crates/hiveweb/src/services/agent_hook.rs`：
  - `create_hook(pool, agent_id, actor_role, meta) -> Result<AgentHook, AppError>`：校验权限（FR-017/FR-018）、校验每触发点 ≤ 5 个（FR-001）、校验引用有效性（FR-006）、校验 URL 合法性（FR-007）、校验 HTTP headers 无注入（FR-007b，拒绝 `\r`/`\n`，name 仅 `[a-zA-Z0-9_-]+`）
  - `update_hook(pool, agent_id, hook_id, actor_role, meta) -> Result<AgentHook, AppError>`：乐观锁校验 + 权限校验
  - `delete_hook(pool, agent_id, hook_id, actor_role) -> Result<(), AppError>`：权限校验
  - `list_hooks(pool, agent_id) -> Result<Vec<AgentHook>, AppError>`：按 trigger_point + sort_order 排序
  - `load_hooks_for_agent(pool, agent_id) -> Result<HashMap<String, Vec<AgentHook>>, AppError>`：给 orchestrator 用，按 trigger_point 分组
- [x] T021 [US1] 在 `crates/hiveweb/src/services/mod.rs` 中声明 `pub mod agent_hook;`

### 后端 — API 层

- [x] T022 [P] [US1] 实现 Hook REST API handlers 在 `crates/hiveweb/src/api/agent_hook.rs`：
  - `POST /api/agents/:id/hooks` — 创建 Hook
  - `GET /api/agents/:id/hooks` — 列出 Agent 全部 Hook
  - `PUT /api/agents/:id/hooks/:hook_id` — 更新 Hook
  - `DELETE /api/agents/:id/hooks/:hook_id` — 删除 Hook
  - 所有 handler 从 JWT claims 提取 actor 角色进行 RBAC 校验
- [x] T023 [US1] 在 `crates/hiveweb/src/api/mod.rs` 中注册 Hook 路由，挂载到现有的 admin 路由组下（`/api/agents/:id/hooks`），应用 admin JWT 鉴权中间件

### 后端 — Runtime 层（Hook 执行引擎）

- [x] T024 [P] [US1] 创建 `crates/hiveweb/src/runtime/hook.rs`：
  - `HookContext` struct：含 `agent_id`、`identifier`、`session_id`、`actor_id`、`request_id`、`trigger_point`、`timestamp`
  - `pub async fn run_hooks(pool, hooks, point, ctx, deps) -> Result<(), HookError>`：核心执行循环
  - `async fn execute_hook_action(hook, ctx, deps) -> Result<(), HookError>`：分发到三种动作类型
  - `fn trace_hook_exec(hook, ctx, outcome, error_kind, elapsed_ms)`：输出仅含白名单错误分类的结构化 tracing，不写数据库
  - `HookError` enum：Timeout / ActionFailed(String) / WebhookFailed(String)
- [x] T025 [US1] 在 `crates/hiveweb/src/runtime/mod.rs` 中声明 `pub mod hook;`

### 后端 — Orchestrator 集成

- [x] T026 [US1] 修改 `build_agent_context()` 在 `crates/hiveweb/src/runtime/orchestrator.rs`：在加载 Agent 配置后，调用 `agent_hook::load_hooks_for_agent(pool, agent_id)` 加载 hooks 并存入 `AgentContext`
- [x] T027 [US1] 在 `AgentContext` struct 中添加 `hooks: HashMap<String, Vec<AgentHook>>` 字段
- [x] T028 [US1] 在 orchestrator 中插入 `run_hooks()` 调用（7 个触发点），每个插入点按错误处理策略分类：
  - **阻塞模式可中止主流程的触发点**（`before_agent_start` / `before_llm_call` / `before_tool_call`）：
    - `let result = run_hooks(...).await; if let Err(e) = result { return ... }` — 阻塞模式失败时中止 Agent 流程
  - **仅 tracing、不可回滚的触发点**（`after_llm_call` / `after_tool_call` / `after_agent_end`）：
    - `let _ = run_hooks(...).await;` — 无论成功失败都忽略，继续主流程（因为 after 阶段不能回滚已完成操作）
  - **错误处理触发点**（`on_agent_error`）：
    - `let _ = run_hooks(...).await;` — 即使 on_agent_error Hook 失败，也不能阻止错误事件发出
    - **关键守卫**：调用 `run_hooks()` 前需检查 `ctx.hooks` 是否为空。若 hooks 尚未加载（错误发生在 `build_agent_context()` 之前），跳过不触发（与 spec Edge Case "on_agent_error 提前失败路径"对齐）。
  - 具体插入位置：
    - `before_agent_start`：在 system_prompt 组装 + tools schema 构建之后、`provider.chat_stream_with_retry()` 之前（当前 L214 附近）— **阻塞模式可中止**
    - `after_agent_end`：在 `finalize_with_variant()` 中 `append_assistant_message` 之后、`emit done` 之前（当前 L442-L447 之间）— **仅 tracing**
    - `on_agent_error`：在每个 `emit_error()` 调用之后（L182/L231/L341/L382）— **仅 tracing**；L173 处因 hooks 未加载，加 `ctx.hooks.is_empty()` 守卫跳过
    - `before_tool_call`：在 `handle_workspace_tool()` capability 鉴权之前（L970-987）— **阻塞模式可中止**
    - `after_tool_call`：在 tool result emit + persist 之后（L311-L330 之后）— **仅 tracing**
    - `before_llm_call`：在 `provider.chat_stream_with_retry()` 调用之前（L214 之前）— **阻塞模式可中止**
    - `after_llm_call`：在 `resp.is_error()` 检查之后、assistant content 处理之前（L225 之后）— **仅 tracing**

### 前端 — Agent Hook 编辑面板

- [x] T029 [P] [US1] 创建 `web-admin/src/services/agentHook.ts`：Hook CRUD API 客户端（createHook / updateHook / deleteHook / listHooks），复用现有 `api.ts` 的 axios instance 和 auth header；不暴露执行历史客户端
- [x] T030 [P] [US1] 创建 `web-admin/src/components/AgentHookEditor/AgentHookEditor.tsx`：
  - 展示 Agent 当前已配置的 Hook 列表（表格：名称、触发点、动作类型、启用状态、操作按钮）
  - "添加 Hook"按钮 → 打开 HookFormModal
  - 每行操作：启用/禁用开关、编辑、删除（含确认弹窗）
  - 按 trigger_point 分组显示 + sort_order 排序
- [x] T031 [P] [US1] 创建 `web-admin/src/components/AgentHookEditor/HookFormModal.tsx`：
  - 表单字段：名称（Input）、描述（TextArea）、触发点（Select，7 选项）、动作类型（Select，3 选项）、动作参数（动态表单，按动作类型切换）
  - `call_function` 参数：function 下拉选择 + args JSON 编辑器
  - `call_workflow` 参数：workflow 下拉选择 + args JSON 编辑器
  - `http_webhook` 参数：URL 输入框 + headers 键值对编辑器 + timeout_ms 输入框
  - 高级选项：阻塞模式开关 + 自定义超时时间（InputNumber）
  - 保存时调用 API client，校验字段完整性
- [x] T032 [US1] 在 Agent 编辑页 `web-admin/src/pages/AgentEdit.tsx`（或对应页面）中集成 AgentHookEditor 组件：
  - 作为 Agent 编辑表单的新 Tab 页或折叠面板区域
  - main Agent 时 Hook 编辑区域不渲染（FR-017：仅 Super 可配，且 UI 不展示）
  - 权限不足时禁用"添加 Hook"按钮并显示提示

**Checkpoint**: US1 完成 — 管理员可完整地为 Agent 配置 Hook 并验证运行时触发。

---

## Phase 4: User Story 2 — HTTP Webhook 与外部系统集成 (Priority: P2)

**Goal**: Hook 支持 HTTP Webhook 动作类型，包括 URL 校验、SSRF 防护、失败重试。

**Independent Test**: 配置 Webhook Hook → 触发对话 → 外部端点收到 POST 请求。

### 后端 — Webhook 执行

- [x] T033 [P] [US2] 在 `crates/hiveweb/src/runtime/hook.rs` 中实现 `execute_webhook()` 函数：
  - 构造 POST payload（含 `agent_identifier`、`session_id`、`trigger_point`、`timestamp`）
  - 复用现有 SSRF 校验逻辑（检查 URL host 不是内网 IP / 云 metadata 端点）
  - 仅允许 `https://` 协议（FR-007）
  - 可选自定义 headers（从 `action_params.headers` 读取）
  - 超时 = hook.timeout_ms 或默认 10s（FR-011）
- [x] T034 [US2] 在 `crates/hiveweb/src/runtime/hook.rs` 中实现 `retry_webhook()` 后台重试函数：
  - 最多重试 3 次，间隔 1s / 2s / 4s 指数退避（FR-007a）
  - 通过 `tokio::spawn` 异步执行，不阻塞 Hook 串行流；任务不持有数据库连接
  - 每次重试独立计时（不计入 Hook 整体超时）
  - 每次尝试、最终成功或失败均输出不含 URL、payload、响应体或原始错误文本的结构化 tracing
- [x] T035 [US2] 在 `execute_hook_action()` 中集成 Webhook 分支：首次调用 `execute_webhook()`，失败则 `tokio::spawn(retry_webhook(...))` 并立即返回 Ok（不阻塞后续 Hook）

### 前端 — Webhook 配置 UI

- [x] T036 [US2] 在 HookFormModal 中完善 `http_webhook` 动作类型参数表单：
  - URL 输入框带前端 `https://` 格式校验
  - Headers 键值对动态编辑器（添加/删除行）
  - 超时时间 InputNumber（默认 10000ms）

**Checkpoint**: US2 完成 — Webhook Hook 可配置、触发、重试，并可通过 tracing 排障。

---

## Phase 5: User Story 3 — Hook 调用 Workflow/Function 实现自动化 (Priority: P2)

**Goal**: Hook 可调用平台已有 Function/Workflow（只读观察者），复用现有 invoker 和 workflow executor。

**Independent Test**: 配置 `before_agent_start` Hook（call_function: `format_template`）→ 触发对话 → 验证调用成功并输出 tracing，数据库无新增执行记录。

### 后端 — Function/Workflow 调用

- [x] T037 [P] [US3] 在 `crates/hiveweb/src/runtime/hook.rs` 中实现 `execute_function()`：
  - 从 `action_params.function_id` 解析 Function
  - 构造入参：合并 `action_params.args` + HookContext 字段
  - 通过 `Invoker` 或 `builtins::lookup()` 执行（沿用现有 capability 鉴权）
  - `chat_respond` 内置 Function 禁止调用（返回 error）
- [x] T038 [P] [US3] 在 `crates/hiveweb/src/runtime/hook.rs` 中实现 `execute_workflow()`：
  - 从 `action_params.workflow_id` 解析 Workflow
  - 通过 `WorkflowExecutor` 执行
  - 构造入参：合并 `action_params.args` + HookContext 字段
- [x] T039 [US3] 在 `execute_hook_action()` 中集成 Function 和 Workflow 分支

### 前端 — Function/Workflow 选择

- [x] T040 [US3] 在 HookFormModal 中完善 `call_function` 和 `call_workflow` 动作参数表单：
  - Function 下拉列表（过滤掉 `chat_respond`）
  - Workflow 下拉列表
  - 入参 JSON 编辑器（Monaco Editor 或 TextArea）

**Checkpoint**: US3 完成 — Function/Workflow 调用 Hook 可用。

---

## Phase 6: 2026-07-16 Requirement Supersession — tracing-only

**Goal**: 避免高频 Hook 执行历史造成数据库容量压力；运行结果仅由 tracing 承载。

- [x] T059 [P] 在 `crates/hiveweb/src/runtime/hook.rs` 中移除主执行及 Webhook 重试的所有数据库写入，改为包含 outcome、elapsed_ms、request_id 等字段的脱敏 tracing
- [x] T060 [P] 从 `crates/hiveweb/src/api/agent_hook.rs`、`models/agent_hook.rs`、`services/agent_hook.rs` 移除执行历史路由、DTO、查询和清理逻辑
- [x] T061 [P] 从 `web-admin/src/pages/AgentPage.tsx` 和 `services/agentHook.ts` 移除执行历史入口与客户端，并删除 `HookExecutionLog.tsx`
- [x] T062 [P] 保留 V023 迁移及既有数据，不新增 DROP 或数据清理迁移
- [x] T063 同步规格并完成后端、前端与静态检查

**Superseded historical tasks**: 原 T018–T019、T041–T044 的执行历史查询合同，以及原 T046 的保留期清理合同，均由本阶段取代，不再属于产品功能。

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: 跨 user story 的完善工作。

- [x] T045 [P] 在 `crates/hiveweb/src/runtime/hook.rs` 中输出结构化 tracing：字段包含 `agent_id`、`agent_identifier`、`hook_id`、`hook_name`、`session_id`、`trigger_point`、`action_type`、`outcome`、`elapsed_ms`、`request_id`；仅记录白名单 `error_kind`，不记录任意下游错误文本
- [x] T047 [P] 在 `crates/hiveweb/src/api/agent_hook.rs` 中为 `call_function`/`call_workflow` 类型 Hook 的 `GET /api/agents/:id/hooks` 响应中附带目标 Function/Workflow 的名称（便于前端展示引用摘要）
- [x] T048 [P] 前端错误提示中文统一：在 `web-admin/src/utils/error_messages.ts` 中新增 6001-6006 错误码的用户文案映射
- [x] T049 [P] 添加 `HOOK_WEBHOOK_RETRY_MAX` 到 `crates/hiveweb/.env.example`；不提供执行历史保留期配置
- [ ] T050 运行 `quickstart.md` 验证：完整走通配置 Hook → 触发对话 → 查看 tracing，并确认执行历史数据库行数不变
- [x] T051 [P] 在 `crates/hiveweb/src/services/agent_hook.rs` 的 `create_hook`/`update_hook` 中实现 FR-007b HTTP header 校验：
  - 拒绝 header name/value 中包含 `\r` 或 `\n` 字符
  - header name 仅允许 `[a-zA-Z0-9_-]+` 字符集，不合格返回 6003 并提示具体违规字段
- [x] T054 [P] 性能基准测试：编写 benchmark 验证 Hook 调度开销 p95 ≤ 50ms（SC-003），不含 action 本身耗时 — 对 `run_hooks()` 空钩子列表压测
- [x] T055 [P] 持久化回归测试：任意 Hook 执行及 Webhook 重试均不新增 `hook_executions` 行（SC-006）

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 (Setup)**: No dependencies — 可立即开始
- **Phase 2 (Foundational)**: Depends on Phase 1 — **阻塞所有 user story**
- **Phase 2.5 (红灯测试)**: 可与 Phase 2 并行 — 必须观察到 FAIL 后才能实现
- **Phase 3 (US1)**: Depends on Phase 2 + Phase 2.5 红灯测试 FAIL → MVP 入口
- **Phase 4 (US2)**: Depends on Phase 2 — 可与 US1 并行（依赖 `runtime/hook.rs` 骨架）
- **Phase 5 (US3)**: Depends on Phase 2 — 可与 US1/US2 并行（依赖 `runtime/hook.rs` 骨架）
- **Phase 6 (tracing-only supersession)**: Depends on Phase 3–5，移除旧执行历史合同
- **Phase 7 (Polish)**: Depends on all three user stories 完成

### User Story Dependencies

- **US1 (P1)**: Foundational 完成后可开始。无其他 story 依赖。
- **US2 (P2)**: Foundational 完成后可开始。依赖 `runtime/hook.rs` 中的 `execute_hook_action()` 分发框架（US1 提供）。
- **US3 (P2)**: Foundational 完成后可开始。依赖 `runtime/hook.rs` 中的 `execute_hook_action()` 分发框架（US1 提供）。

### Within Each User Story

- Tests (Phase 2.5) MUST 先写且 FAIL → 再实现
- Models → Services → API handlers → Runtime → Orchestrator 集成 → Frontend
- 后端完整后再做前端（或并行分配前后端开发者）

### Parallel Opportunities

- T001–T002（Setup）可并行
- T003（配置 Migration）与历史 V023 兼容确认可并行
- T006–T007（Error codes + Model）可并行
- T008–T017, T052–T058（所有红灯测试）可并行
- T020 + T022（Service + API handlers 可部分并行：先定义 trait 接口）
- T029–T031（前端组件）可与后端实现并行
- T033–T035（US2 后端）和 T037–T039（US3 后端）可并行（不同函数，同文件但可分支开发）
- T045、T047–T049、T051、T054–T055（Polish 任务）全部可并行

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Phase 1: Setup（T001–T002）
2. Phase 2: Foundational（T003–T007）**← CRITICAL BLOCKER**
3. Phase 2.5: US1 红灯测试（T008–T013）→ 观察 FAIL
4. Phase 3: US1 Implementation（T020–T032）
5. **STOP & VALIDATE**: 独立测试 US1 — 配置 Hook → 触发对话 → 验证执行 + tracing，确认数据库无执行历史新增行
6. Deploy/demo if ready

### Incremental Delivery

1. Foundational → 基础设施就绪
2. US1 → 独立测试 → Deploy/Demo（**MVP!**）
3. US2 → 独立测试 → Deploy/Demo
4. US3 → 独立测试 → Deploy/Demo
5. tracing-only supersession → 移除执行历史存储/API/UI
6. Polish → 生产就绪

---

## Notes

- [P] tasks = 不同文件，无未完成依赖
- [Story] label 映射到具体 user story 以追踪
- 每个 user story 必须可独立完成和测试
- 红灯测试必须 FAIL 后才能开始实现
- 每个 task 或逻辑组完成后提交
- 在任何 checkpoint 停下验证 story 独立性
- Hook 超时配置 `HOOK_TIMEOUT_MS` 默认 10000ms
- Webhook 重试次数 `HOOK_WEBHOOK_RETRY_MAX` 默认 3
- 错误码 6001-6006 不与现有 1xxx-5xxx 冲突
