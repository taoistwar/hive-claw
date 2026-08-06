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

> ✅ **TDD 流程**：执行历史移除先由 T056–T058 观察红灯，再修改生产代码。T012、T015、T017、T053 需要真实 Assistant/Hook 基础设施，当前保持 Pending；不得用空的 `#[ignore]` 测试宣称完成。

### US1 红灯测试 — Hook 配置 CRUD + 执行

- [x] T008 [P] [US1] 契约测试：`POST /api/agents/:id/hooks` 创建 Hook → 验证返回 200 + Hook 对象含 id/created_at
- [x] T009 [P] [US1] 契约测试：`GET /api/agents/:id/hooks` 列出 Agent 全部 Hook → 验证返回数组 + 按 sort_order 排序
- [x] T010 [P] [US1] 契约测试：`PUT /api/agents/:id/hooks/:hook_id` 更新 Hook → 验证 optimistic lock (4094) 行为
- [x] T011 [P] [US1] 契约测试：`DELETE /api/agents/:id/hooks/:hook_id` 删除 Hook → 验证返回 200 + 再次 GET 列表不含该 Hook
- [ ] T012 [P] [US1] 集成测试：配置 `before_agent_start` Hook（http_webhook）→ 调用 `POST /api/assistant` → 验证 Hook 执行且 `hook_executions` 不新增记录（需要真实 MySQL、Redis、外部用户库、LLM 与有效 Agent）
- [x] T013 [P] [US1] 集成测试：同一触发点配置 6 个 Hook → 验证保存时返回 6001 错误码
- [ ] T053 [P] [US1] 集成测试：配置 `before_agent_start` 阻塞 Hook → 调用 `POST /api/assistant` → 验证超时返回 6004/HTTP 408、其他失败返回 6005/HTTP 500、配额恰好回滚一次、`after_agent_end` 未执行且没有 assistant 占位消息 (SC-007)

### US2 红灯测试 — HTTP Webhook

- [x] T014 [P] [US2] 契约测试：`POST /api/agents/:id/hooks`（action_type=http_webhook, webhook_url=http://...）→ 验证返回 6003（非 HTTPS）
- [ ] T015 [P] [US2] 集成测试：配置 Webhook Hook 指向不可达 URL → 调用 `POST /api/assistant` → 验证失败与重试不会写入 `hook_executions`（需要可控 HTTPS 测试端点）
- [x] T052 [P] [US2] 契约测试：POST Hook 的 `action_params.headers` 含 `\r\n` header injection → 验证返回 6003（FR-007b）

### US3 红灯测试 — Function/Workflow 调用

- [x] T016 [P] [US3] 契约测试：`POST /api/agents/:id/hooks`（action_type=call_function, function_id 不存在）→ 验证返回 6002
- [ ] T017 [P] [US3] 集成测试：配置 `after_agent_end` Hook（call_workflow）→ 调用 `POST /api/assistant` → 验证 workflow 被执行（需要 Assistant 基础设施 + 有效 Workflow；当前无可执行测试）

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
  - `create_hook(pool, agent_id, actor_role, meta) -> Result<AgentHook, AppError>`：校验权限（FR-017/FR-018）、校验每触发点 ≤ 5 个（FR-001）、校验引用有效性（FR-006）、校验 URL/DNS 公网目标（FR-007）、用实际 HTTP parser 校验 headers 并拒绝 `Host` 覆盖（FR-007b）
  - `update_hook(pool, agent_id, hook_id, actor_role, meta) -> Result<AgentHook, AppError>`：先合并并校验有效 action type/params，再执行乐观锁与更新；仅修改 `action_params` 也不得绕过出站策略
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
  - `HookContext` struct：含 `agent_id`、`identifier`、`session_id`、`actor_id`、`request_id`、`trigger_point`、`message`、`channel`、`client_type`、`client_version`；Webhook payload 的 `timestamp` 在发送时生成
  - `pub async fn run_hooks(pool, hooks, point, ctx, deps) -> Result<(), HookError>`：核心执行循环
  - `async fn execute_hook_action(hook, ctx, deps) -> Result<(), HookError>`：分发到三种动作类型
  - `fn trace_hook_exec(hook, ctx, outcome, error_kind, elapsed_ms)`：输出仅含白名单错误分类的结构化 tracing，不写数据库
  - `HookError` enum：Timeout / ActionFailed(String) / WebhookFailed(String)
- [x] T025 [US1] 在 `crates/hiveweb/src/runtime/mod.rs` 中声明 `pub mod hook;`

### 后端 — Orchestrator 集成

- [x] T026 [US1] 修改 `services::agent::fetch_content_from_db()`：加载 Agent 配置时调用 `agent_hook::load_hooks_for_agent(pool, agent_id)`，按 trigger point 分组后存入 `AgentContent.hooks`
- [x] T027 [US1] 在 `AgentContent` struct 中添加 `hooks: HashMap<String, Vec<AgentHook>>` 字段，供 orchestrator 为每个成功 hop 固定快照
- [x] T028 [US1] 在 orchestrator 中插入 `run_hooks()` 调用（7 个触发点），每个插入点按错误处理策略分类：
  - **阻塞模式可中止主流程的触发点**（`before_agent_start` / `before_llm_call` / `before_tool_call`）：
    - `let result = run_hooks(...).await; if let Err(e) = result { return ... }` — 阻塞模式失败时中止 Agent 流程；Timeout 映射 6004，BlockingFailed 映射 6005；先执行 tracing-only `on_agent_error`，不得进入正常 finalize
  - **仅 tracing、不可回滚的触发点**（`after_llm_call` / `after_tool_call` / `after_agent_end`）：
    - `let _ = run_hooks(...).await;` — 无论成功失败都忽略，继续主流程（因为 after 阶段不能回滚已完成操作）
  - **错误处理触发点**（`on_agent_error`）：
    - `run_hooks(...).await` 的结果只写结构化 tracing；即使 on_agent_error Hook 失败，也不能覆盖 `POST /api/assistant` 的原始同步 JSON 错误
    - **关键守卫**：仅在已有 `LoadedAgentHooks` 时调用 `on_agent_error`。首次 `fetch_content()` 失败且快照尚未加载时跳过；后续 fetch 失败使用最近一次成功快照（与 spec Edge Case "on_agent_error 提前失败路径"对齐）。
  - 具体插入位置：
    - `before_agent_start`：在 system_prompt 组装 + tools schema 构建之后、`provider.chat_stream_with_retry()` 之前（当前 L214 附近）— **阻塞模式可中止**
    - `after_agent_end`：仅在正常 `finalize_with_variant()` 路径、最终扩展收集和 assistant 消息持久化之前执行 — **仅 tracing**；阻塞 Hook 失败路径跳过
    - `on_agent_error`：在错误确定后、同步 JSON 响应返回前执行 — **仅 tracing**；hooks 未加载时以空集合守卫跳过
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
  - 复用 `network.http` 的 URL/metadata/IP 策略，要求 DNS 全部答案均为公网地址，并对通过校验的地址做 DNS pinning
  - 仅允许 `https://` 协议（FR-007）
  - 禁用环境代理与重定向；可选自定义 headers 使用保存期同一 parser 且不得覆盖 `Host`
  - 超时 = hook.timeout_ms 或默认 10s（FR-011）
- [x] T034 [US2] 在 `crates/hiveweb/src/runtime/hook.rs` 中实现 `retry_webhook()` 后台重试函数：
  - 最多重试 3 次，使用固定 1s / 2s / 4s 退避表（FR-007a）
  - 通过 `tokio::spawn` 异步执行，不阻塞 Hook 串行流；任务不持有数据库连接
  - 每次重试独立计时（不计入 Hook 整体超时）
  - 每次重试重新执行相同的 URL/DNS 全答案校验、pinning、禁代理/重定向与 header 策略；仅 `ResolveUnavailable`、reqwest `is_connect()` 与 attempt timeout 重试，`Policy` / header / client build / 其他 Request / HTTP non-2xx 均停止
  - 每次尝试、最终成功或失败均输出不含 URL、payload、响应体或原始错误文本的结构化 tracing
- [x] T035 [US2] 在 `execute_hook_action()` 中集成 Webhook 分支：首次调用 `execute_webhook()` 后同步 typed 分类；仅 eligible transient failure 执行 `tokio::spawn(retry_webhook(...))`，不可重试错误直接返回安全失败且不阻塞后续非阻塞 Hook

### 前端 — Webhook 配置 UI

- [x] T036 [US2] 在 HookFormModal 中完善 `http_webhook` 动作类型参数表单：
  - URL 输入框带前端 `https://` 格式校验
  - Headers 键值对动态编辑器（添加/删除行）
  - 超时时间 InputNumber（默认 10000ms）

**Checkpoint**: US2 完成 — Webhook Hook 可配置、触发、重试，并可通过 tracing 排障。

---

## Phase 5: User Story 3 — Hook 调用 Workflow/Function 实现自动化 (Priority: P2)

**Goal**: Hook 可调用平台已有 Function/Workflow，读取 `_agent_context` 快照并通过受控 `_agent_context_updates` 更新当前运行的 `AgentContext`；复用现有 invoker、workflow executor 与 capability 权限边界。

**Independent Test**: 配置 `before_agent_start` Hook（call_function: `format_template`）→ 触发对话 → 验证调用成功并输出 tracing，数据库无新增执行记录。

### 后端 — Function/Workflow 调用

- [x] T037 [P] [US3] 在 `crates/hiveweb/src/runtime/hook.rs` 中实现 `execute_function()`：
  - 从 `action_params.function_id` 解析 Function
  - 构造入参：合并 `action_params.args` + HookContext 字段
  - 通过 `Invoker` 或 `builtins::lookup()` 执行（沿用现有 capability 鉴权）
  - `chat_respond` 内置 Function 禁止调用（返回 error）
  - 将只读 `_agent_context` 快照最后注入 Function 输入并覆盖 `action_params.args` 中的同名字段；仅对返回值中的 `_agent_context_updates` 调用受控 `apply_agent_context_updates`
- [x] T038 [P] [US3] 在 `crates/hiveweb/src/runtime/hook.rs` 中实现 `execute_workflow()`：
  - 从 `action_params.workflow_id` 解析 Workflow
  - 通过 `WorkflowExecutor` 执行
  - 构造入参：合并 `action_params.args` + HookContext 字段
  - Workflow 节点输出中的 `_agent_context_updates` 在执行层内同步到当前 `AgentContext`；所有公共 end-output 及 HTTP `node_results` 每节点顶层移除该内部字段，显式 output schema 声明该字段时以 5005/HTTP 422 拒绝
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

## Phase 6.5: 2026-07-23 Assistant 同步错误语义回归

- [x] T064 [P] [US1] 在 `crates/hiveweb/src/api/chat_assistant.rs` 增加非 ignored 直接测试：6004/HTTP 408、6005/HTTP 500、普通文本 message、无效内部错误降级 5000，并以注入计数器验证错误响应路径只调用一次配额回滚
- [x] T065 [US1] 在 `crates/hiveweb/src/runtime/orchestrator.rs` 使用强类型内部错误通道；阻塞 Hook 失败执行 tracing-only `on_agent_error` 后立即返回，跳过 `after_agent_end` 和 assistant 消息持久化

**Remaining infrastructure coverage**: T012、T015、T017、T053 保持 Pending，待真实 Assistant/Hook 基础设施测试完成后才能勾选。

---

## Phase 6.6: 2026-07-23 终止错误 Hook 覆盖与总预算

- [x] T066 [P] [US1] 在 `crates/hiveweb/src/runtime/orchestrator.rs` 增加终止错误变体映射和暂停时间单元测试：覆盖 Agent 内容加载、模型预设、Provider、阻塞 Hook、路由循环和最大跳数错误；证明 `on_agent_error` 使用单一 30 秒总预算，阶段失败或预算耗尽后原始错误保持不变
- [x] T067 [US1] 将已加载 Hook 快照后的终止错误收敛到统一 `terminate_agent_error` 路径；在固定 30 秒总预算内执行 tracing-only `on_agent_error`，预算耗尽只记录安全 tracing，不递归、不覆盖原始错误；首次 Agent 内容加载失败时明确跳过
- [x] T068 [US1] 保持阻塞 Hook 的 6004/6005、配额单次回滚、跳过 `finalize_with_variant` / `after_agent_end` / assistant 占位消息语义，并同步 spec、plan、contracts 与 checklists

---

## Phase 6.7: 2026-07-23 per-hop Hook 快照与缓存一致性

- [x] T069 [P] [US1] 增加非基础设施单元测试与源码接线契约，证明 Hook mutation 成功时缓存失效恰好调用一次，mutation 失败时不调用且保留原始错误，并验证 create/update/delete 均在数据库写入之后接入统一失效 helper
- [x] T070 [US1] 为 Hook create/update/delete service 增加成功后 best-effort `agent:content:{id}` 缓存失效；失败仅记录固定 `redis_delete_failed` tracing，不覆盖已提交的 mutation 结果
- [x] T071 [US1] 将 spec、plan、research、tasks、contracts、quickstart 与 checklists 统一为 per-successful-Agent-hop 快照：hop 内固定、下一 hop 重新 fetch、后续加载失败使用最近成功快照、首次失败跳过

---

## Phase 6.8: 2026-07-24 显式执行上下文与 sticky tracing-only

- [x] T072 [P] [US1] 在 `crates/hiveweb/src/services/runtime_audit.rs` 增加聚焦测试：普通 `RuntimeExecutionContext` 保留 request/session 并可入 best-effort DB 队列；`for_hook()` 保留关联 ID、禁止 DB 入队且重复派生仍为 tracing-only
- [x] T073 [P] [US1] 在 `crates/hiveweb/src/runtime/execution_context.rs` 增加 compile-fail 合同，证明 persistence mode 不能通过 `Default` 或 `Deserialize` 从外部 payload 构造；在 `runtime_audit.rs` 增加源码接线合同，覆盖 Assistant → Orchestrator → Hook → Workflow → Plugin → Capability 的显式传播且禁止 task-local 隐式状态
- [x] T074 [US1] 在 `crates/hiveweb/src/api/chat_assistant.rs`、`runtime/orchestrator.rs`、`runtime/hook.rs`、`runtime/workflow/workflow_executor.rs`、`runtime/workflow/function_node.rs`、`runtime/invoker.rs` 与 `runtime/capability.rs` 实现并传播显式 `RuntimeExecutionContext`：Assistant 构造携带 request/session 的普通 best-effort 上下文；Orchestrator 在 Hook 边界调用 sticky `for_hook()`；下游只克隆或传递受限上下文，所有 audit tracing 保留关联 ID 且 Hook 子链不得重新启用 DB audit
- [x] T075 [P] [US1] 将 `spec.md`、`plan.md`、`research.md`、`contracts/api.md`、`quickstart.md` 与 `tasks.md` 同步为显式上下文边界；保留 2026-07-16 Hook tracing-only 决议，并明确 tracing-only 不禁止业务或配置所需的正常数据库访问

---

## Phase 6.9: 2026-07-24 受控 `AgentContext` 更新语义

- [x] T076 [US3] 审阅 `crates/hiveweb/src/runtime/hook.rs` 与 `runtime/workflow/workflow_executor.rs` 的现有接线：Function/Workflow 合并 `action_params.args`，运行时 `_agent_context` 覆盖配置同名字段；仅将顶层 `_agent_context_updates` 映射到当前内存 `AgentContext` 的 records、extensions 与字符串 metadata；Workflow 节点即时应用 updates，所有公共 end-output 路径移除内部字段，output schema 禁止声明该保留键
- [x] T077 [P] [US3] 同步 `spec.md`、`plan.md`、`research.md`、`data-model.md`、`contracts/api.md`、`quickstart.md`、`tasks.md` 与 checklists：将 2026-06-02 “严格只读观察者”标记为被取代的历史决议，明确受控 updates 不直接修改持久化 Agent 配置、system prompt、数据库或 sticky tracing-only 审计模式

---

## Phase 6.10: 2026-07-24 可信输入与 Webhook 出站边界

- [x] T078 [P] [US2] 先增加聚焦红灯测试：Function/Workflow HookContext
  覆盖伪造 args；共享出站策略拒绝私网/metadata/混合 DNS、禁止 redirect/proxy
  并固定 DNS；header 拒绝非法形态与 `Host`；仅 `action_params` 更新仍验证；
  resolver 使用 typed `Policy` / `ResolveUnavailable`；Hook 仅重试
  `ResolveUnavailable`、reqwest `is_connect()` 与 attempt timeout，
  Policy/header/client build/其他 Request/HTTP non-2xx 均不重试；
  Request-ID 拒绝非 canonical/nil UUID 且原始值不进入 tracing
- [x] T079 [US2] 在 `runtime/hook.rs`、`runtime/capabilities/network_http.rs`
  与 `services/agent_hook.rs` 实现统一安全路径：固定
  `args < HookContext < _agent_context`，Webhook 保存、首次发送和每次重试均
  使用 HTTPS、DNS 全答案公网校验、DNS pinning、禁代理/重定向与共享 header
  parser
- [x] T080 [P] [US1] 在 `middleware/request_id.rs` 将 HTTP 关联边界收敛为
  固定 36 字节非 nil hyphenated UUID，统一小写；缺失/非法时生成新 UUID，响应、
  RuntimeExecutionContext、HookContext 与 tracing 只传播规范化值

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: 跨 user story 的完善工作。

- [x] T045 [P] 在 `crates/hiveweb/src/runtime/hook.rs` 中输出结构化 tracing：字段包含 `agent_id`、`hook_id`、`session_id`、`trigger_point`、`action_type`、`outcome`、`elapsed_ms`、`request_id`；仅记录白名单 `error_kind`，不记录可识别的 Agent/Hook 名称、任意下游错误文本或原始 update payload
- [x] T047 [P] 在 `crates/hiveweb/src/api/agent_hook.rs` 中为 `call_function`/`call_workflow` 类型 Hook 的 `GET /api/agents/:id/hooks` 响应中附带目标 Function/Workflow 的名称（便于前端展示引用摘要）
- [x] T048 [P] 前端错误提示中文统一：在 `web-admin/src/utils/error_messages.ts` 中新增 6001-6006 错误码的用户文案映射
- [x] T049 [P] 添加 `HOOK_WEBHOOK_RETRY_MAX` 到 `crates/hiveweb/.env.example`；不提供执行历史保留期配置
- [ ] T050 运行 `quickstart.md` 验证：完整走通配置 Hook → 触发对话 → 查看 tracing，并确认执行历史数据库行数不变
- [x] T051 [P] 在 `crates/hiveweb/src/services/agent_hook.rs` 的 `create_hook`/`update_hook` 中实现 FR-007b HTTP header 校验：
  - 要求 `headers` 为字符串键值对象，使用实际 HTTP header parser 拒绝 CR/LF 与非法 name/value
  - 拒绝 `Host` 覆盖；不合格统一返回不回显原始 header 的静态 6003 文案
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
- **Phase 6.8 (explicit execution context)**: Depends on Phase 6，显式保留关联 ID 并保证 Hook 子链 sticky tracing-only
- **Phase 6.9 (controlled AgentContext updates)**: Depends on Phase 5–6.8，统一既有运行时更新能力、权限边界与 tracing-only 审计语义
- **Phase 6.10 (trusted input/outbound boundary)**: Depends on Phase 4–6.9，统一 HookContext、Webhook SSRF 和 Request-ID 边界
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
