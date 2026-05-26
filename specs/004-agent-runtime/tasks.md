---

description: "Task list for Agent Runtime (Capability-based WASM plugin runtime)"
---

# Tasks: Agent Runtime（Capability-based WASM Plugin Runtime）

**Input**: Design documents from `/specs/004-agent-runtime/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: 🔴 测试为强制要求（宪法 Principle II — NON-NEGOTIABLE，吸取 003 偏离 1 教训）。Phase 2.5 红灯测试必须先于对应实现任务编写并提交。

**Organization**: 按 user story 分组，每个 story 可独立交付与验证。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: 可并行（不同文件，无未完成依赖）
- **[Story]**: 所属用户故事（US1..US7）
- 描述中必须含**确切**文件路径

<!--
  ============================================================================
  User Story 来源（spec.md）：
  - US1 (P1) MVP 🎯 — WASM Plugin 管理
  - US2 (P1)        — Function / Tool / Skill 管理
  - US4 (P1)        — Capability 鉴权（与 US1/US2 同等重要）
  - US3 (P2)        — Workflow DAG
  - US5 (P2)        — Agent 多层编排 + 路由
  - US6 (P2)        — 测试聊天（SSE）
  - US7 (P3)        — Category / Tag
  ============================================================================
-->

---

## Phase 1: Setup（共享基础设施）

**Purpose**: workspace 依赖、迁移目录、配置文件骨架。

- [ ] T001 [P] 添加后端依赖到 `crates/hiveweb/Cargo.toml`：`extism = "1"`、`http-body-util`（已加测试）、`once_cell`、`jsonschema = "0.17"`、`regex`
- [ ] T002 [P] 添加 workspace 依赖：`crates/hiveweb` 在 `[dependencies]` 中引用 `agent = { path = "../agent" }`、`providers = { path = "../providers" }`、`skills = { path = "../skills" }`
- [ ] T003 [P] 添加前端依赖到 `web/package.json`：`reactflow ^11`、`monaco-editor ^0.45` (用于 system_prompt 编辑) 、`@monaco-editor/react`
- [ ] T004 [P] 创建配置文件骨架 `crates/hiveweb/llm_presets.toml.example`，含 `default = "cheap-fast"` + 一两个示例 preset（primary + fallback）
- [ ] T005 [P] 创建 examples 目录 `examples/plugins/README.md`，说明 Rust PDK 编写 + 编译流程
- [ ] T149 [P] 创建 `crates/hiveweb/.env.example` 集中声明 004 新增 env var（在 003 已有基础上），共 15 项：
  - **Plugin 上传 & 调用**：`PLUGIN_MAX_BYTES=16777216`、`PLUGIN_CALL_TIMEOUT_MS=30000`、`PLUGIN_CALL_MAX_MEMORY_MB=128`、`PLUGIN_CALL_FUEL=10000000000`
  - **Instance Pool**：`PLUGIN_POOL_MAX_PER_PLUGIN=8`、`PLUGIN_POOL_MAX_TOTAL=64`、`PLUGIN_POOL_IDLE_TIMEOUT_SEC=600`、`PLUGIN_POOL_ACQUIRE_TIMEOUT_MS=5000`
  - **Agent / Chat**：`AGENT_MAX_HOPS=5`、`CHAT_RETENTION_DAYS=30`、`CHAT_SSE_MAX_CONCURRENT_PER_ADMIN=2`
  - **LLM / Audit**：`LLM_PRESETS_PATH=./llm_presets.toml`、`LLM_NODE_TIMEOUT_MS=25000`、`LLM_CHAIN_TIMEOUT_MS=45000`、`AUDIT_RETENTION_DAYS=90`
  DEPLOYMENT.md 同步更新；每个 env 加注释指向对应 FR / research 段。

---

## Phase 2: Foundational（阻塞所有 user story）

**Purpose**: 建表 + runtime 骨架 + 工厂。所有 user story 实现都依赖此。

**⚠️ CRITICAL**: 此阶段完成前不能动 user story 实现，但 Phase 2.5 红灯测试可与本阶段并行起草。

### 数据库迁移（V008..V018）

- [ ] T006 [P] 创建 V008 capabilities 表 `crates/hiveweb/migrations/V008__capabilities.sql`（按 data-model §V008）
- [ ] T007 [P] 创建 V009 categories 表 `crates/hiveweb/migrations/V009__categories.sql`
- [ ] T008 [P] 创建 V010 tags + taggings 表 `crates/hiveweb/migrations/V010__tags.sql`
- [ ] T009 [P] 创建 V011 plugins 表 `crates/hiveweb/migrations/V011__plugins.sql`（含 FULLTEXT 索引）
- [ ] T010 [P] 创建 V012 functions 表 `crates/hiveweb/migrations/V012__functions.sql`
- [ ] T011 [P] 创建 V013 workflows + nodes + edges 表 `crates/hiveweb/migrations/V013__workflows.sql`
- [ ] T012 [P] 创建 V014 tools + skills 表 `crates/hiveweb/migrations/V014__tools_skills.sql`（注意 skills 是 markdown 模式）
- [ ] T013 [P] 创建 V015 agents + agent_tools + agent_skills + agent_permissions 表 `crates/hiveweb/migrations/V015__agents.sql`（含 model_preset 列）
- [ ] T014 [P] 创建 V016 chat_sessions + chat_messages 表 `crates/hiveweb/migrations/V016__chat.sql`（chat_sessions 含 `admin_phone_snapshot` / `admin_nickname_snapshot` 快照列，admin 删除后保留发起者追溯，data-model 不变量 #12）
- [ ] T015 [P] 创建 V017 runtime_audit_logs 表 `crates/hiveweb/migrations/V017__runtime_audit_logs.sql`
- [ ] T016 [P] 创建 V018 seed main agent + 5 内置 function（kind=1） `crates/hiveweb/migrations/V018__seed.sql`
- [ ] T017 在 `crates/hiveweb/src/bin/migrate.rs` 中注册 V008–V018 entries（不可并行：修改同一文件）

### Models（与 V008..V018 行映射）

- [ ] T018 [P] Capability/Category/Tag 模型 `crates/hiveweb/src/models/capability.rs`、`models/category.rs`、`models/tag.rs`
- [ ] T019 [P] Plugin 模型 `crates/hiveweb/src/models/plugin.rs`（含 `deleted_at`、`sha256`、`s3_key`）
- [ ] T020 [P] Function 模型 `crates/hiveweb/src/models/function.rs`（含 kind、plugin_id、schemas JSON）
- [ ] T021 [P] Workflow/WorkflowNode/WorkflowEdge 模型 `crates/hiveweb/src/models/workflow.rs`
- [ ] T022 [P] Tool 模型 `crates/hiveweb/src/models/tool.rs`
- [ ] T023 [P] Skill 模型（markdown 模式：content + frontmatter）`crates/hiveweb/src/models/skill.rs`
- [ ] T024 [P] Agent 模型 `crates/hiveweb/src/models/agent.rs`（含 model_preset、parent_agent_id、depth）
- [ ] T025 [P] ChatSession/ChatMessage 模型 `crates/hiveweb/src/models/chat.rs`
- [ ] T026 [P] RuntimeAuditLog 模型 `crates/hiveweb/src/models/runtime_audit_log.rs`

### Runtime 骨架（不依赖具体 capability handler）

- [ ] T027 [P] Capability 静态注册表骨架 `crates/hiveweb/src/runtime/capability.rs`（定义 `Capability` struct、`CapabilityRegistry`、未实现具体 handler；列出 11 个 capability name 常量）
- [ ] T028 [P] Instance Pool 骨架 `crates/hiveweb/src/runtime/pool.rs`（`PluginPool` + `PluginGuard`，归还前 `Plugin::reset()` 调用占位，未接 Extism）
- [ ] T029 [P] Plugin 调用 invoker 骨架 `crates/hiveweb/src/runtime/invoker.rs`（resolve plugin → acquire from pool → invoke → audit；先 stub `invoke()` 返回 unimplemented）
- [ ] T030 [P] Workflow 执行器骨架 `crates/hiveweb/src/runtime/workflow.rs`（拓扑 BFS + tokio::join_all 的接口，先 stub）
- [ ] T031 [P] Agent 编排骨架 `crates/hiveweb/src/runtime/agent.rs`（接 `crates/agent::AgentRunner` 占位）
- [ ] T032 [P] LLM adapter `crates/hiveweb/src/runtime/llm.rs`（启动期读 `llm_presets.toml` → 构造 `HashMap<LlmPresetName, Arc<dyn LLMProvider>>`；提供 `provider_for(&Agent)`）
- [ ] T033 在 `crates/hiveweb/src/runtime/mod.rs` 暴露上述子模块，并 `pub use` 关键类型
- [ ] T034 在 `crates/hiveweb/src/lib.rs` 增加 `pub mod runtime;`
- [ ] T035 扩展 `AppState`（`crates/hiveweb/src/api/mod.rs`）含 `runtime_state: Arc<RuntimeState>`，启动期**严格按 plan §Startup Initialization Order 的 12 步顺序**注入：env → migrations → capability registry upsert → builtin function upsert → custom function 索引 → llm_presets 加载 → ToolRegistry 装配 → SubagentManager/MemoryStore → Pool 空池 → router → 后台任务 → HTTP listen；任一前置失败 panic 退出码 1

### Reactflow + Monaco 资源接入（前端）

- [ ] T036 [P] 在 `web/src/main.tsx` 注册 react-flow 样式与 Monaco worker；新建 `web/src/utils/reactflow.ts` 公共配置

### 跨切：乐观锁（spec §Edge Cases 并发编辑）

- [ ] T150 创建 `crates/hiveweb/src/services/optimistic_lock.rs`：提供 `check_and_bump(table, id, client_updated_at) -> Result<(), AppError::OptimisticLockConflict>` helper；返回 4094 错误码
- [ ] T151 在 Plugin/Workflow/Tool/Skill/Agent service 的 update 路径（T070/T108/T082/T083/T116）调用 `optimistic_lock::check_and_bump`；client 必须在 PUT 请求体携带 `updated_at`，service 层校验后再执行 UPDATE
- [ ] T152 在 Plugin/Workflow/Tool/Skill/Agent API（T071/T109/T085/T086/T117）的 PUT handler 中反序列化 `updated_at` 字段并传给 service

**Checkpoint**：基础完成 — Phase 2.5 与各 user story 可并行启动

---

## Phase 2.5: Tests (Red Phase) — 强制先于实现

**Purpose**: 吸取 003 偏离 1 教训，所有契约/集成/组件测试必须先红灯后绿灯。

**⚠️ CRITICAL**: 这些任务必须先以红灯状态提交（CI 标记失败），然后才允许动对应实现任务。

### 后端契约测试（HTTP REST + host_call ABI + SSE）

- [ ] T037 [P] Contract test `crates/hiveweb/tests/contract_plugin.rs`：上传成功 / 列表 / 三维检索 / 详情；**安全 case**：① 非 WASM magic 文件 → 4001；② 超 `PLUGIN_MAX_BYTES` → 4001；③ WASM imports 段含未注册 host function → 拒绝上传（FR-005 v7）；④ 软删除时被未删除 Function 引用 → 4093 + race window 防护（并发新建 Function 一定一方失败）
- [ ] T038 [P] Contract test `crates/hiveweb/tests/contract_function.rs`：CRUD + JSON Schema 校验 + builtin 拒删
- [ ] T039 [P] Contract test `crates/hiveweb/tests/contract_workflow.rs`：CRUD + DAG PUT graph + execute
- [ ] T040 [P] Contract test `crates/hiveweb/tests/contract_tool.rs`：CRUD + kind 与 function/workflow 引用约束 + `kind=1` schema 深度等值校验（输入故意差 1 个字段 → 5002）+ `kind=2` workflow input_schema 兼容性校验
- [ ] T041 [P] Contract test `crates/hiveweb/tests/contract_skill.rs`：CRUD（markdown content + frontmatter，无 schema）
- [ ] T042 [P] Contract test `crates/hiveweb/tests/contract_agent.rs`：CRUD + 树结构 + depth ≤ 10 + main 限 Super + model_preset 验证（含 5007）
- [ ] T043 [P] Contract test `crates/hiveweb/tests/contract_chat.rs`：session CRUD + POST messages 走 SSE，断言 `Content-Type: text/event-stream`
- [ ] T044 [P] Contract test `crates/hiveweb/tests/contract_category_tag.rs`：CRUD + 引用阻塞
- [ ] T045 [P] Contract test `crates/hiveweb/tests/contract_capability_list.rs`：GET `/api/capabilities` 与 `GET /api/agents/model-presets` 形状

### 后端集成测试（Capability 鉴权 / Workflow 拓扑 / Agent 路由 / Pool / 多版本 Plugin）

- [ ] T046 [P] Integration test `crates/hiveweb/tests/it_capability_denied.rs`：Agent 无 `network.http` capability 时 host_call 返 4030；每个 capability 一条 case
- [ ] T047 [P] Integration test `crates/hiveweb/tests/it_capability_unknown.rs`：未注册 capability 名返 4040
- [ ] T048 [P] Integration test `crates/hiveweb/tests/it_subagent_no_inherit.rs`：父 Agent 有 `network.http`，子 Agent 没有 → 子 Agent 调用拒绝（FR-003 v3）
- [ ] T049 [P] Integration test `crates/hiveweb/tests/it_pool_reset.rs`：并发 16 个 Plugin 调用，命中池后归还前 reset，第二轮调用观测无状态泄漏
- [ ] T050 [P] Integration test `crates/hiveweb/tests/it_pool_timeout.rs`：① Plugin 调用超 `PLUGIN_CALL_TIMEOUT_MS` 强制中止 → 5004 + 丢弃实例（cache_misses +1）；② Pool 满时 acquire 超 `PLUGIN_POOL_ACQUIRE_TIMEOUT_MS` → 5009 `PoolBusy`；③ fuel-based 中断（构造纯 CPU 死循环 plugin）→ 5004
- [ ] T051 [P] Integration test `crates/hiveweb/tests/it_workflow_cycle.rs`：保存有环 DAG → 4092 拒绝
- [ ] T052 [P] Integration test `crates/hiveweb/tests/it_workflow_execute.rs`：3 节点 DAG（含并行分支）执行结果断言
- [ ] T053 [P] Integration test `crates/hiveweb/tests/it_workflow_node_failure.rs`：节点失败 → 整个 workflow 终止 + 5005
- [ ] T054 [P] Integration test `crates/hiveweb/tests/it_agent_route_loop.rs`：构造 A→B→A→B 路由 > 5 跳 → hard-rule 终止
- [ ] T055 [P] Integration test `crates/hiveweb/tests/it_agent_depth_limit.rs`：在第 10 层下新建 → 5006 拒绝
- [ ] T056 [P] Integration test `crates/hiveweb/tests/it_agent_main_protected.rs`：DELETE main → 5001；非 Super 修改 main → 2001
- [ ] T057 [P] Integration test `crates/hiveweb/tests/it_dangerous_capability_super_only.rs`：System 角色保存含 `db.execute` 的 Agent → 拒绝
- [ ] T058 [P] Integration test `crates/hiveweb/tests/it_plugin_ref_block.rs`：被 Function 引用的 Plugin → DELETE 返 4093
- [ ] T059 [P] Integration test `crates/hiveweb/tests/it_plugin_multi_version.rs`：同 identifier 不同 version 共存；Function.plugin_id 绑定具体版本
- [ ] T060 [P] Integration test `crates/hiveweb/tests/it_llm_fallback.rs`：primary provider mock 返错 → 自动切到 fallback；全链失败 → SSE error
- [ ] T061 [P] Integration test `crates/hiveweb/tests/it_optimistic_lock.rs`：两个 client 并发改同一 Workflow，第二个 PUT 用旧 `updated_at` → 409
- [ ] T062 [P] Integration test `crates/hiveweb/tests/it_audit_log.rs`：执行一次 host_call 后 `runtime_audit_logs` 应有一行（含 request_id、capability、outcome）

### 前端组件测试（Vitest）

- [ ] T063 [P] Component test `web/src/components/__tests__/PluginUploader.test.tsx`：拒绝 > 16 MB、显示 sha256 计算进度
- [ ] T064 [P] Component test `web/src/components/__tests__/DagEditor.test.tsx`：拖拽 + 连线 + 环检测红框
- [ ] T065 [P] Component test `web/src/components/__tests__/ChatStream.test.tsx`：覆盖全部 6 种 SSE 事件渲染 — `token`（增量文本）/ `tool_call`（卡片显示 tool 名 + 入参摘要）/ `tool_result`（卡片折叠）/ `routed`（"已切换到 {agent}" 分隔条）/ `fallback_used`（"已切换备用模型" 提示）/ `done`（最终消息 + elapsed_ms）/ `error`（错误码文案，与 done 互斥）；额外测"等待首字"占位 + 30s 超时 + 连接中断"重新发送"按钮
- [ ] T066 [P] Component test `web/src/components/__tests__/ModelPresetSelect.test.tsx`：下拉显示 `/agents/model-presets` 返回项，未知 preset 警告
- [ ] T067 [P] Component test `web/src/components/__tests__/CapabilityPicker.test.tsx`：dangerous 标记 + Super 才能勾选
- [ ] T068 [P] Component test `web/src/components/__tests__/SchemaEditor.test.tsx`：非法 JSON Schema 时显示报错并阻止保存
- [ ] T069 [P] Component test `web/src/components/__tests__/a11y.test.tsx` 扩展：覆盖 PluginPage / WorkflowPage / ChatPage / AgentPage axe 检测（SC-008）

**Checkpoint**：33 红灯任务必须先全部 CI 失败提交。随后实现任务方可启动。

---

## Phase 3: User Story 1 — WASM Plugin 管理（P1）🎯 MVP

**Goal**：管理员上传 / 编辑 / 软删除 / 三维检索 Plugin。

**Independent Test**：上传 WASM → 列表可见 → 编辑 metadata → 关联 category/tags → 软删除 → 引用阻塞验证。

### 后端服务层 + API

- [ ] T070 [US1] Plugin service `crates/hiveweb/src/services/plugin.rs`：
  - **upload**：校验 WASM magic bytes (`\0asm`) → 校验大小 ≤ `PLUGIN_MAX_BYTES` → 扫 imports 段拒绝非宿主注册的 host function（FR-005 v7）→ 计算 sha256 → S3 put → DB insert
  - **update metadata only**：仅允许改 name/description/category/tags/author/repository_url；走乐观锁（client 携带 updated_at，不一致返 4094）
  - **soft delete with reference check**（spec SC-009 v7 race window 防护）：
    - 开 transaction
    - `SELECT * FROM plugins WHERE id = ? FOR UPDATE`（行锁）
    - `SELECT COUNT(*) FROM functions WHERE plugin_id = ? AND ...`（同事务内）
    - 任何引用 → ROLLBACK + 4093
    - 否则 UPDATE plugins SET deleted_at = NOW() + COMMIT
  - **加载前 sha256 重校验**（FR-029 v7）：从 S3 GET WASM 后重算 sha256 比对 DB；不一致 → 拒绝实例化 + audit + tracing::error 通知运维
- [ ] T071 [US1] Plugin API `crates/hiveweb/src/api/plugin.rs`：`GET/POST/PUT/DELETE /api/plugins`，multipart upload 处理；三维检索 SQL（FULLTEXT + category + tags JOIN）
- [ ] T072 [P] [US1] 在 `crates/hiveweb/src/storage/s3.rs` 增加 `put_wasm(key, bytes) -> Result<()>` 与 `delete_wasm(key)` 帮助函数
- [ ] T073 [US1] 把 plugin 路由 mount 到 `crates/hiveweb/src/api/mod.rs::create_router`

### 前端

- [ ] T074 [P] [US1] `web/src/services/plugin.ts`：CRUD + multipart upload + 三维检索
- [ ] T075 [P] [US1] `web/src/components/PluginUploader.tsx`：文件选取 / sha256 预览 / 大小校验
- [ ] T076 [P] [US1] `web/src/components/PluginFilters.tsx`：category 树 + tags 多选 + search
- [ ] T077 [US1] `web/src/pages/PluginPage.tsx`：列表 + 抽屉编辑 + 引用阻塞确认对话框
- [ ] T078 [US1] `web/src/hooks/usePlugins.ts`：分页 + filter 状态
- [ ] T157 [P] [US1] [SC-008] axe 检测 `web/src/components/__tests__/a11y_plugin.test.tsx`：覆盖 PluginPage / PluginUploader / PluginFilters，0 critical/serious

**Checkpoint**：Plugin CRUD + 检索 + 软删除可独立演示。T037 / T058 / T059 / T063 / T157 应该转绿。

---

## Phase 4: User Story 2 — Function / Tool / Skill（P1）

**Goal**：定制 Function 关联 Plugin export；内置 Function 不可删；Tool 注册到 ToolRegistry；Skill 是 markdown。

**Independent Test**：注册一个定制 Function（绑定 Plugin export）→ 包装成 Tool → 创建 Skill markdown → 在 Agent 编辑器里可选；ToolRegistry 启动时含全部 builtin + 已注册 custom。

### 后端 — Builtin Function

- [ ] T079 [P] [US2] 5 个 builtin function 实现 `crates/hiveweb/src/runtime/builtins.rs`：`format.template` / `json.parse` / `json.stringify` / `text.regex_match` / `chat.respond`，每个实现 `agent::Tool` trait
- [ ] T080 [US2] 启动期把 builtins 注入 `agent::ToolRegistry` 与 DB `functions` 表（idempotent upsert by identifier，kind=1）

### 后端 — Function/Tool/Skill CRUD

- [ ] T081 [P] [US2] Function service `crates/hiveweb/src/services/function.rs`：custom CRUD + Plugin 引用校验 + JSON Schema 校验（draft 2020-12）
- [ ] T082 [P] [US2] Tool service `crates/hiveweb/src/services/tool.rs`：CRUD + schema 一致性校验（data-model 不变量 #11）— `kind=1` 时 tools.input_schema / output_schema **深度 JSON 等值校验** 与引用 function 的 schema，不一致返 5002 `Schema mismatch`；`kind=2` 时 tools.input_schema 必须能赋值给 workflow 入口 function 的 input_schema（必含所有 required 字段且类型一致）
- [ ] T083 [P] [US2] Skill service `crates/hiveweb/src/services/skill.rs`：CRUD（markdown content + 解析 frontmatter）
- [ ] T084 [P] [US2] Function API `crates/hiveweb/src/api/function.rs`
- [ ] T085 [P] [US2] Tool API `crates/hiveweb/src/api/tool.rs`
- [ ] T086 [P] [US2] Skill API `crates/hiveweb/src/api/skill.rs`
- [ ] T087 [US2] 把以上 3 个路由 mount 到 router；启动期把 DB 中所有 custom Tool 注册到 `agent::ToolRegistry`（重启时重建）
- [ ] T088 [US2] runtime invoker `invoker.rs`：实现 custom Function 调用（按 Function.plugin_id 解析 plugin → pool.acquire → Plugin::call(export, payload)）；超时 / 内存上限按 env 限定

### 前端

- [ ] T089 [P] [US2] `web/src/services/function.ts` / `tool.ts` / `skill.ts`
- [ ] T090 [P] [US2] `web/src/components/SchemaEditor.tsx`：Monaco JSON 编辑 + 实时校验
- [ ] T091 [P] [US2] `web/src/components/SkillMarkdownEditor.tsx`：Monaco markdown + frontmatter
- [ ] T092 [P] [US2] `web/src/pages/FunctionPage.tsx`：列表（区分 builtin / custom）+ 编辑抽屉
- [ ] T093 [P] [US2] `web/src/pages/ToolPage.tsx`
- [ ] T094 [P] [US2] `web/src/pages/SkillPage.tsx`
- [ ] T158 [P] [US2] [SC-008] axe 检测 `web/src/components/__tests__/a11y_function_tool_skill.test.tsx`：覆盖 FunctionPage / ToolPage / SkillPage / SchemaEditor / SkillMarkdownEditor，0 critical/serious

**Checkpoint**：US1 + US2 联合可演示；T038–T041 / T068 / T158 转绿。

---

## Phase 5: User Story 4 — Capability 鉴权（P1）

**Goal**：宿主在 host_call 入口拦截所有越权与未知 capability 调用；超时 / 内存上限强制；审计 100% 覆盖。

**Independent Test**：Agent 无 `network.http` → Plugin 调 `network.http` → 立即 4030；未注册 capability → 4040；30s 长 Plugin → 5004；audit log 写入。

### Capability handlers

- [ ] T095 [P] [US4] `network.http` handler `crates/hiveweb/src/runtime/capabilities/network_http.rs`：白名单域名 + 4 MB body cap + 并发 8/Plugin
- [ ] T096 [P] [US4] `fs.read` / `fs.write` handlers `crates/hiveweb/src/runtime/capabilities/fs.rs`：限定 `/tmp/plugin/` 前缀
- [ ] T097 [P] [US4] `s3.read` / `s3.write` handlers `crates/hiveweb/src/runtime/capabilities/s3.rs`：复用 storage::s3
- [ ] T098 [P] [US4] `db.query` / `db.execute` handlers `crates/hiveweb/src/runtime/capabilities/db.rs`：从 `named_queries.toml` 加载预注册查询；自由 SQL 永远拒绝
- [ ] T099 [P] [US4] `llm.invoke` handler `crates/hiveweb/src/runtime/capabilities/llm.rs`：走 `runtime/llm::provider_for(&agent)`
- [ ] T100 [P] [US4] `secret.get` handler `crates/hiveweb/src/runtime/capabilities/secret.rs`：env-backed allowlist；Super 才能为 Agent 授予
- [ ] T101 [P] [US4] `time.now` / `log.emit` handlers `crates/hiveweb/src/runtime/capabilities/utility.rs`

### Dispatcher + Audit

- [ ] T102 [US4] 完成 `runtime/capability.rs::dispatch(agent_ctx, envelope_bytes)`：deserialize envelope → 查 Agent.permissions → unknown → 4040 → handler → audit → 返回 envelope
- [ ] T103 [US4] 完成 `runtime/pool.rs`：与 Extism 集成（`Plugin::new_with_manifest` + linear_memory 上限 + fuel-based timeout）
- [ ] T104 [US4] 完成 `runtime/invoker.rs::invoke()`：把 host_call 注册成 Extism Host Function，闭包内 capture `dispatcher`
- [ ] T105 [US4] Audit 写入 `services/runtime_audit.rs::record()`：填 request_id / agent_id / plugin_id / capability / outcome / elapsed_ms / payload_summary（脱敏）

### 前端

- [ ] T106 [P] [US4] `web/src/services/capability.ts`：GET capabilities
- [ ] T107 [P] [US4] `web/src/components/CapabilityPicker.tsx`：含 dangerous 标记；非 Super 不可勾选 dangerous（hidden = 不渲染）
- [ ] T162 [P] [US4] Runtime metrics 端点 `crates/hiveweb/src/api/runtime.rs`：`GET /api/runtime/pool/stats` 返回 global + per_plugin Pool 快照（in_use / idle / created_total / cache_misses / wait_count / reset_failures），role ≥ System；前端 `web/src/pages/DashboardPage.tsx` 加 "Plugin Pool 健康度" 卡片显示 cache_misses 与 wait_count 是否异常

**Checkpoint**：US1+US2+US4 联合演示完整 capability 鉴权链路；T046–T050 / T057 / T062 / T067 转绿。

---

## Phase 6: User Story 3 — Workflow DAG（P2）

**Goal**：管理员在 Web 上拖拽 Function 形成 DAG；保存校验环；执行按拓扑 + 并行。

**Independent Test**：3 节点 DAG（A→B、A→C、B→D、C→D）→ 执行返回 D 输出。

- [ ] T108 [P] [US3] Workflow service `crates/hiveweb/src/services/workflow.rs`：CRUD + cycle 检测（DFS） + mapping schema 校验。`workflow_edges.mapping` JSON 形如 `{"dst.input.<field>": "<src_node_key>.output.<path>"}`；保存时校验：① src_node_key 在 workflow 内存在；② src.output.<path> 在该 Function 的 output_schema 中存在；③ 类型可赋值给 dst.input.<field>；④ 所有 dst Function 的 required input 都有上游 mapping 或外部入参；任一不满足 → 5005 `Workflow node input mapping invalid`
- [ ] T109 [P] [US3] Workflow API `crates/hiveweb/src/api/workflow.rs`：含 `PUT /:id/graph` 与 `POST /:id/execute`
- [ ] T110 [US3] 完成 `runtime/workflow.rs::execute()`：拓扑分层 + `tokio::join_all` 并行 + 节点级 timeout + 错误短路
- [ ] T111 [US3] 把 Workflow 注册成 `agent::Tool`（Workflow-wrapped Tool）：当 `tools.kind=2` 时，runtime 创建一个 Tool impl，内部 dispatch 到 workflow execute
- [ ] T112 [P] [US3] `web/src/services/workflow.ts`
- [ ] T113 [P] [US3] `web/src/components/DagEditor/DagEditor.tsx`：reactflow 集成 + 自定义 node renderer（Function picker）+ edge mapping 配置
- [ ] T114 [P] [US3] `web/src/components/DagEditor/CycleDetector.ts`：客户端环检测红框预警
- [ ] T115 [US3] `web/src/pages/WorkflowPage.tsx`
- [ ] T159 [P] [US3] [SC-008] axe 检测 `web/src/components/__tests__/a11y_workflow.test.tsx`：覆盖 WorkflowPage / DagEditor（含 reactflow 自定义 node renderer），重点验证 keyboard navigation；0 critical/serious

**Checkpoint**：可拼装 Workflow 并执行；T039 / T051 / T052 / T053 / T064 / T159 转绿。

---

## Phase 7: User Story 5 — Agent 多层编排（P2）

**Goal**：Agent 树（main + 多层专家）+ LLM 路由 + hard-rule 兜底。

**Independent Test**：main 配 `coding-expert` 子 Agent → 问"Rust 异步"→ SSE 看到 routed 事件 → 最终 rust-expert 回复。

- [ ] T116 [P] [US5] Agent service `crates/hiveweb/src/services/agent.rs`：CRUD + tree + depth 校验（违反 → 5006）+ main protection（DELETE → 5001；非 Super 修改 → 2001）+ dangerous capability 授予的 Super 校验 + model_preset 存在性校验（启动加载的 preset 集合；未命中 → 5007 `ModelPresetUnknown`）
- [ ] T117 [P] [US5] Agent API `crates/hiveweb/src/api/agent.rs`：含 `GET /api/agents/model-presets`
- [ ] T118 [US5] `runtime/agent.rs`：把 Agent 的 tools/skills/permissions 组装为 `agent::AgentRunner` 的 context（system_prompt = Agent.system_prompt + 拼接的 Skill markdown；ToolRegistry 子集 = Agent.tools）
- [ ] T119 [US5] `runtime/agent.rs::route()`：把 `route_to_subagent(agent_id, reason)` 注册为一个特殊 Tool；hard-rule 检查 depth、loop 跳次（`AGENT_MAX_HOPS`）、capability denial；越界返回 SSE error 而非 LLM 自决
- [ ] T120 [US5] `runtime/llm.rs::load_presets()`：解析 `llm_presets.toml`，调用 `providers::make_provider` 构造 primary，再用 `providers::FallbackProvider::new` 套上 fallback 链；标记 default
- [ ] T121 [P] [US5] `web/src/services/agent.ts`
- [ ] T122 [P] [US5] `web/src/components/AgentTree.tsx`：层级树展示，含 depth 限制提示
- [ ] T123 [P] [US5] `web/src/components/ModelPresetSelect.tsx`：下拉 + GET /agents/model-presets
- [ ] T124 [P] [US5] `web/src/components/AgentEditor.tsx`：tools / skills 多选 + permissions（CapabilityPicker）+ system_prompt（Monaco）+ model_preset
- [ ] T125 [US5] `web/src/pages/AgentPage.tsx`
- [ ] T160 [P] [US5] [SC-008] axe 检测 `web/src/components/__tests__/a11y_agent.test.tsx`：覆盖 AgentPage / AgentTree / AgentEditor / ModelPresetSelect / CapabilityPicker，0 critical/serious

**Checkpoint**：路由功能可用；T042 / T048 / T054 / T055 / T056 / T060 / T066 / T160 转绿。

---

## Phase 8: User Story 6 — 测试聊天（SSE）（P2）

**Goal**：管理员在 Web 上输入文字，看到流式增量回复，可见 routed 事件。

**Independent Test**：输入"你好"→ SSE token 流 → done；输入触发路由的问题 → routed → 子 Agent token → done。

- [ ] T126 [P] [US6] Chat service `crates/hiveweb/src/services/chat.rs`：session CRUD + message persist + history 拉取
- [ ] T127 [US6] Chat API `crates/hiveweb/src/api/chat.rs`：`POST /api/chat/sessions/:id/messages` 返回 `Sse<impl Stream<Item = Event>>`
  - **响应 headers**（production-critical 防 reverse proxy 缓冲）：`Cache-Control: no-cache, no-transform` / `Connection: keep-alive` / `X-Accel-Buffering: no`（axum::response::sse::Sse::keep_alive 配合自定义 headers）
  - **Keep-alive**：每 15s 发送 SSE comment `: ping\n\n` 维持连接（绕过中间代理 30s 默认空闲超时）
  - **事件类型**（6 种）：`token` / `tool_call` / `tool_result` / `routed` / `fallback_used` / `done`（`error` 与 `done` 互斥作为流终点）
  - **会话所有权校验**（FR-027 v7）：JWT.admin_id == session.admin_id；不匹配 → 403；Super 例外但仍走显式判定
  - **并发限制**（CHK232）：bypass `RateLimit` 中间件；用独立计数器：同一 admin 同时活跃 SSE 流 > `CHAT_SSE_MAX_CONCURRENT_PER_ADMIN`（默认 2）→ 立即返 429 + code 4291
- [ ] T128 [US6] `runtime/agent.rs::run_session()`：把 chat 历史 + 用户最新消息喂给 `AgentRunner`，event-loop 推送 SSE 事件；中断断开时 user message 已 persist
- [ ] T129 [P] [US6] `web/src/services/chat.ts`：EventSource wrapper（解析 token / tool_call / routed / done / error）
- [ ] T130 [P] [US6] `web/src/components/ChatStream.tsx`：6 SSE 事件渲染（`token` 增量、`tool_call`/`tool_result` 卡片对、`routed` 切换分隔条、`fallback_used` 模型切换提示、`done` 终态、`error` 错误展示）；"正在思考…" 占位（提交后到首事件之间）；30s 无响应超时降级；SSE 中途断开 → "连接中断 + 重新发送" 按钮（user 消息已 persist，assistant 中断内容不写库）
- [ ] T131 [US6] `web/src/pages/ChatPage.tsx`：会话列表 + 当前对话区域
- [ ] T132 [P] [US6] cron 任务 `crates/hiveweb/src/bin/chat_retention.rs`：按 `CHAT_RETENTION_DAYS` 删超期 session（级联清 message）
- [ ] T161 [P] [US6] [SC-008] axe 检测 `web/src/components/__tests__/a11y_chat.test.tsx`：覆盖 ChatPage / ChatStream，0 critical/serious
- [ ] T163 [P] [US6] Integration test `crates/hiveweb/tests/it_sse_concurrency.rs`：同一 admin 启 3 个 SSE 流（`CHAT_SSE_MAX_CONCURRENT_PER_ADMIN=2`），第 3 次必返 429 + code 4291；关闭其中 1 个后第 3 个能成功建立

**Checkpoint**：完整对话可演示；T043 / T065 / T161 转绿；SC-010 端到端可测。

---

## Phase 9: User Story 7 — Category / Tag（P3）

- [ ] T133 [P] [US7] Category service + API `crates/hiveweb/src/services/category.rs` + `api/category.rs`
- [ ] T134 [P] [US7] Tag service + API `crates/hiveweb/src/services/tag.rs` + `api/tag.rs`：删除时检查 taggings 引用
- [ ] T135 [P] [US7] `web/src/services/category.ts` + `web/src/services/tag.ts`
- [ ] T136 [P] [US7] `web/src/pages/CategoryPage.tsx`（树形）+ `web/src/pages/TagPage.tsx`

**Checkpoint**：T044 转绿。

---

## Phase 10: Polish & Cross-Cutting Concerns

- [ ] T137 [P] 完成 `llm_presets.toml` 实际配置 + 文档化（README in `crates/hiveweb/`）
- [ ] T138 [P] 编写 `examples/plugins/weather/` 完整可编译的 Rust PDK 例子（含 README）
- [ ] T139 [P] [SC-008] a11y 总集回归：CI 整合 T157+T158+T159+T160+T161 + Category/Tag 页面，0 critical/serious 跨所有页面（每个 US 内的 a11y 任务已分散到各自 Phase 末尾）
- [ ] T140 [P] [SC-004] perf bench host_call dispatch：`crates/hiveweb/benches/host_call.rs`，目标 p95 ≤ 5 ms
- [ ] T141 [P] [SC-005] perf bench Plugin call 命中池：`crates/hiveweb/benches/plugin_invoke.rs`，命中 p95 ≤ 50 ms，冷启动 ≤ 300 ms
- [ ] T142 [P] [Principle IV] EXPLAIN 关键查询（plugins FULLTEXT + agents tree + chat_messages by session）→ 写入 `specs/004-agent-runtime/perf-evidence.md`
- [ ] T153 [P] [SC-001] Plugin 上传性能基准 `crates/hiveweb/benches/plugin_upload.rs`：1 MB Plugin 上传（入库 + S3 PUT）端到端 p95 ≤ 5 秒；结果写入 perf-evidence.md
- [ ] T154 [P] [SC-002] Plugin 列表 + 三维检索基准 `crates/hiveweb/benches/plugin_list.rs`：500 条 Plugin 数据集下 list + filter + FULLTEXT search 各 p95 ≤ 1 秒
- [ ] T155 [P] [SC-003] Workflow 保存 + 校验基准 `crates/hiveweb/benches/workflow_save.rs`：50 节点 DAG 的 PUT graph（含 cycle detection + mapping 校验）p95 ≤ 1 秒
- [ ] T156 [P] [SC-006] Agent 路由决策端到端基准 `crates/hiveweb/benches/agent_route.rs`：含 1 次 LLM 决策调用的路由 p95 ≤ 1.5 秒（mock LLM provider 固定 800ms 响应以隔离 LLM 外部延迟）
- [x] T143 ~~更新 quickstart.md~~ — 已在 analyze 整改阶段直接落地（ModelPreset 下拉、Skill markdown 步骤）
- [x] T144 ~~修订 contracts/api.md Skills~~ — 已在 analyze 整改阶段直接落地（markdown 模式、+ 4094 OptimisticLockConflict、+ 5008 BuiltinSkillProtected）
- [ ] T145 [P] 添加 audit log 保留 cron `crates/hiveweb/src/bin/audit_retention.rs`：默认保留 **90 天**（可配 `AUDIT_RETENTION_DAYS` env，与 spec FR-022 对齐），每日扫描清理超期 `runtime_audit_logs` 行
- [ ] T146 [P] 文档化危险 capability 授予流程到 `specs/004-agent-runtime/SECURITY.md`
- [ ] T147 [P] 在 `web/src/pages/DashboardPage.tsx` 加 runtime 概览卡片（active plugins / today's host_calls / failed calls）
- [ ] T148 [P] CHANGELOG 更新 + tasks.md 标记所有完成项

---

## Dependencies & Execution Order

### Phase 依赖

- Phase 1 → Phase 2 → Phase 2.5 必须先红灯 → Phase 3+
- US1 / US2 / US4 互相基本独立（US2 的 ToolRegistry 注册依赖 US4 的 invoker 完成；US2 的 custom function 调用依赖 US4 的 dispatcher） → 推荐顺序 US1 → US4 → US2，或 US1 / US4 并行后再 US2
- US3 依赖 US2（节点是 Function）
- US5 依赖 US2 + US4（Agent 用 Tool 与 Capability）
- US6 依赖 US5
- US7 独立，可任意 phase 后插入
- Polish 在所有 user story 完成后

### 并行机会

- Phase 2 中 T006–T016 全部 [P]（不同迁移文件）
- Phase 2 中 T018–T026 全部 [P]（不同 model 文件）
- Phase 2.5 中所有 33 个测试 [P]（不同测试文件）
- 各 Phase 内部 [P] 任务可并发开发

---

## Implementation Strategy

### MVP First（仅 US1 + US4 部分）

1. Phase 1 / 2 / 2.5（红灯先）
2. US1 — Plugin 上传 + 列表 + 软删除（不依赖 capability 运行时）
3. **STOP & VALIDATE**：演示 Plugin 上传

### MVP Complete（US1 + US2 + US4）

继续到 capability 鉴权 + Function + Tool + Skill，构成完整可调用闭环。**真正可用的 MVP** 在此节点。

### Incremental Delivery

- 加 US3 Workflow → demo 复杂编排
- 加 US5 Agent 多层 → demo 路由
- 加 US6 SSE chat → demo 完整对话
- 加 US7 Category/Tag → 运营组织能力
- Polish → 性能 / a11y / 文档

---

## Task Summary

- **Total Tasks**: 163（含 analyze v1 整改 T149–T161 + analyze v3 整改 T162 pool/stats + T163 SSE 并发测试）
- **Setup (Phase 1)**: 5 + 1（T149 集中 15 个 env vars）
- **Foundational (Phase 2)**: 31 + 3 跨切乐观锁（T150/T151/T152）= 34
- **Tests (Phase 2.5)**: 33 + 1 SSE 并发（T163）= 34；先红灯
- **US1 Plugin**: 9 + 1 a11y（T157）
- **US2 Function/Tool/Skill**: 16 + 1 a11y（T158）
- **US4 Capability**: 13 + 1 pool/stats 端点（T162）
- **US3 Workflow**: 8 + 1 a11y（T159）
- **US5 Agent**: 10 + 1 a11y（T160）
- **US6 Chat**: 7 + 1 a11y（T161）
- **US7 Category/Tag**: 4
- **Polish (Phase 10)**: 12 + 4 perf bench（T153–T156）= 16；T143/T144 已就地完成

**Parallel Opportunities**: 120+ 任务标 [P]
**Independent MVP**: US1 + US2 + US4（共 38 + 1 metrics 实现任务 + 33 红灯 + 3 乐观锁 + 1 env 配置）

### Analyze v3 整改任务覆盖矩阵（CHK gap 到 task 映射）

| Spec / Doc 更新 | 影响 task | 整改方式 |
| --- | --- | --- |
| 8 个新 env var | T149 | 描述扩写到 15 项 |
| GET /api/runtime/pool/stats 端点 | T162 新增 | US4 加 1 任务 |
| SSE 6 事件（+tool_result/+fallback_used）+ headers + 15s ping | T065, T127, T130 | 描述扩写 |
| SC-009 race window FOR UPDATE | T070 | 描述扩写到 5 个子步骤 |
| WASM imports 静态检查 | T037, T070 | 测试 + 实现描述都扩写 |
| Startup Init Order 12 步 | T035 | 描述扩写 |
| chat_sessions snapshot 列 | T014 | 描述扩写 |
| Tool schema 深度等值 | T040, T082 | 测试 + 实现描述都扩写 |
| 5009 PoolBusy 错误码 | T050 | 测试描述扩写覆盖 |
| 4291 SSE concurrency 错误码 | T163 新增 | US6 加 1 任务 |
| workflow edge mapping schema | T108 | 描述扩写 |

---

## Notes

- [P] = 不同文件、无未完成依赖、可并行
- [Story] = 任务归属用户故事；Setup/Foundational/Polish 不带 Story
- 严格 "Phase 2.5 先红灯" 是不重蹈 003 偏离 1 的硬要求
- 复用 `crates/agent` / `crates/skills` / `crates/providers`，不要再造编排核心
- Skill = markdown 内容，**不**走 OpenAI tool-calling 路径
- 危险 capability 授予 + main Agent 编辑 = Super-only（在 service 层强制 + 前端 hide-if-not-Super）
- `db.execute` / `db.query` 永远走 named query；自由 SQL 0 暴露面
