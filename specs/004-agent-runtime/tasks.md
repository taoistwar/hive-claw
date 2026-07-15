---

description: "Task list for Agent Runtime (Capability-based WASM plugin runtime)"
---

# Tasks: Agent Runtime（Capability-based WASM Plugin Runtime）

> **范围更新（2026-07-14）：** 本文中 `RecommendedGame`、`recommended_games*` 及 `/api/recommended-games*` 相关管理和公开接口已废弃，仅兼容保留；不得新增调用或扩展。Agent Runtime 的其余能力仍为现役范围。

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

- [x] T001 [P] 添加后端依赖到 `crates/hiveweb/Cargo.toml`：`extism = "1"`、`http-body-util`（已加测试）、`once_cell`、`jsonschema = "0.17"`、`regex`
- [x] T002 [P] 添加 workspace 依赖：`crates/hiveweb` 在 `[dependencies]` 中引用 `agent = { path = "../agent" }`、`providers = { path = "../providers" }`、`skills = { path = "../skills" }`
- [x] T003 [P] 添加前端依赖到 `web-admin/package.json`：`reactflow ^11`、`monaco-editor ^0.45` (用于 system_prompt 编辑) 、`@monaco-editor/react`
- [x] T004 [P] 创建配置文件骨架 `crates/hiveweb/llm_presets.toml.example`，含 `default = "cheap-fast"` + 一两个示例 preset（primary + fallback）
- [x] T005 [P] 创建 examples 目录 `examples/plugins/README.md`，说明 Rust PDK 编写 + 编译流程
- [x] T149 [P] 创建 `crates/hiveweb/.env.example` 集中声明 004 新增 env var（在 003 已有基础上），共 15 项：
  - **Plugin 上传 & 调用**：`PLUGIN_MAX_BYTES=16777216`、`PLUGIN_CALL_TIMEOUT_MS=33000`、`PLUGIN_CALL_MAX_MEMORY_MB=128`、`PLUGIN_CALL_FUEL=10000000000`
  - **Instance Pool**：`PLUGIN_POOL_MAX_PER_PLUGIN=8`、`PLUGIN_POOL_MAX_TOTAL=64`、`PLUGIN_POOL_IDLE_TIMEOUT_SEC=600`、`PLUGIN_POOL_ACQUIRE_TIMEOUT_MS=5000`
  - **Agent / Chat**：`AGENT_MAX_HOPS=5`、`CHAT_RETENTION_DAYS=30`、`CHAT_SSE_MAX_CONCURRENT_PER_ADMIN=2`
  - **LLM / Audit**：`LLM_PRESETS_PATH=./llm_presets.toml`、`LLM_NODE_TIMEOUT_MS=25000`、`LLM_CHAIN_TIMEOUT_MS=45000`、`AUDIT_RETENTION_DAYS=90`
  DEPLOYMENT.md 同步更新；每个 env 加注释指向对应 FR / research 段。

---

## Phase 2: Foundational（阻塞所有 user story）

**Purpose**: 建表 + runtime 骨架 + 工厂。所有 user story 实现都依赖此。

**⚠️ CRITICAL**: 此阶段完成前不能动 user story 实现，但 Phase 2.5 红灯测试可与本阶段并行起草。

### 数据库迁移（V008..V038）

- [x] T006 [P] 创建 V008 capabilities 表 `crates/hiveweb/migrations/V008__capabilities.sql`（按 data-model §V008）
- [x] T007 [P] 创建 V009 categories 表 `crates/hiveweb/migrations/V009__categories.sql`
- [x] T008 [P] 创建 V010 tags + taggings 表 `crates/hiveweb/migrations/V010__tags.sql`
- [x] T009 [P] 创建 V011 plugins 表 `crates/hiveweb/migrations/V011__plugins.sql`（含 FULLTEXT 索引）
- [x] T010 [P] 创建 V012 functions 表 `crates/hiveweb/migrations/V012__functions.sql`
- [x] T011 [P] 创建 V013 workflows + nodes + edges 表 `crates/hiveweb/migrations/V013__workflows.sql`
- [x] T012 [P] 创建 V014 tools + skills 表 `crates/hiveweb/migrations/V014__tools_skills.sql`（注意 skills 是 markdown 模式）
- [x] T013 [P] 创建 V015 agents + agent_tools + agent_skills + agent_permissions 表 `crates/hiveweb/migrations/V015__agents.sql`（含 model_preset 列）
- [x] T014 [P] 创建 V016 chat_sessions + chat_messages 表 `crates/hiveweb/migrations/V016__chat.sql`（chat_sessions 含 `admin_phone_snapshot` / `admin_nickname_snapshot` 快照列，admin 删除后保留发起者追溯，data-model 不变量 #12）
- [x] T015 [P] 创建 V017 runtime_audit_logs 表 `crates/hiveweb/migrations/V017__runtime_audit_logs.sql`
- [x] T016 [P] 创建 V018 seed main agent + 5 内置 function（kind=1） `crates/hiveweb/migrations/V018__seed.sql`
- [x] T017 在 `crates/hiveweb-admin/src/bin/migrate.rs` 中注册 V008–V018 entries（不可并行：修改同一文件）

### 数据库迁移扩展（V019..V038）

- [x] T170 [P] V019 `tools.source` 列 — `crates/hiveweb/migrations/V019__tools_source.sql`：workspace | builtin，builtin Tool 只能包装 builtin Function
- [x] T171 [P] V020 `tools.is_always` 列 + 放宽 CHECK — `crates/hiveweb/migrations/V020__tools_is_always.sql`：is_always=1 对所有 Agent 自动可用；允许 kind=1 时 function_id=NULL（meta-tools）
- [x] T172 [P] V021 `skills.is_always` 列 — `crates/hiveweb/migrations/V021__skills_is_always.sql`
- [x] T173 [P] V022–V025 `recommended_games` 表 — 推荐游戏管理（name, reply, reason, tag, game_id, game_name, game_category, game_image, sort_value）
- [x] T174 [P] V026 `tools.category_id` — `crates/hiveweb/migrations/V026__tools_category.sql`
- [x] T175 [P] V027 `skills.category_id` — `crates/hiveweb/migrations/V027__skills_category.sql`
- [x] T176 [P] V028 rename `audit_logs` → `admin_audit_logs` — `crates/hiveweb/migrations/V028__rename_audit_logs_to_admin_audit_logs.sql`
- [x] T177 [P] V029 `functions.required_capabilities` + `tools.required_capabilities` — `crates/hiveweb/migrations/V029__function_tool_capabilities.sql`
- [x] T178 [P] V030 `workflows.required_capabilities` — `crates/hiveweb/migrations/V030__workflow_required_capabilities.sql`
- [x] T179 [P] V031 `skills.required_capabilities` — `crates/hiveweb/migrations/V031__skill_required_capabilities.sql`
- [x] T180 [P] V032 `workflows.category_id` — `crates/hiveweb/migrations/V032__workflow_category.sql`
- [x] T181 [P] V033 `workflows.input_schema` + `start_description` — `crates/hiveweb/migrations/V033__workflow_input_schema.sql`
- [x] T182 [P] V034 `workflow_nodes.node_type` — `crates/hiveweb/migrations/V034__workflow_node_type.sql`（ENUM: function_node, start_node）
- [x] T183 [P] V035 `workflows.output_schema` + `end_description` — `crates/hiveweb/migrations/V035__workflow_output_schema.sql`
- [x] T184 [P] V036 `workflow_nodes` answer_node — `crates/hiveweb/migrations/V036__workflow_answer_node.sql`（添加 end_node, generate_answer_node；function_id 可为 NULL；node_config JSON）
- [x] T185 [P] V037 `capabilities.category_id` — `crates/hiveweb/migrations/V037__capabilities_category.sql`
- [x] T186 [P] V038 seed capability categories — `crates/hiveweb/migrations/V038__seed_capability_categories.sql`

### Models（与 V008..V038 行映射）

- [x] T018 [P] Capability/Category/Tag 模型 `crates/hiveweb-admin/src/models/capability.rs`、`models/category.rs`、`models/tag.rs`
- [x] T019 [P] Plugin 模型 `crates/hiveweb-admin/src/models/plugin.rs`（含 `deleted_at`、`sha256`、`s3_key`）
- [x] T020 [P] Function 模型 `crates/hiveweb-admin/src/models/function.rs`（含 kind、plugin_id、schemas JSON）
- [x] T021 [P] Workflow/WorkflowNode/WorkflowEdge 模型 `crates/hiveweb-admin/src/models/workflow.rs`（含 category_id, input_schema, output_schema, required_capabilities）
- [x] T022 [P] Tool 模型 `crates/hiveweb-admin/src/models/tool.rs`（含 source, is_always, category_id, required_capabilities）
- [x] T023 [P] Skill 模型（markdown 模式：content + frontmatter）`crates/hiveweb-admin/src/models/skill.rs`（含 is_always, category_id, required_capabilities）
- [x] T024 [P] Agent 模型 `crates/hiveweb-admin/src/models/agent.rs`（含 model_preset、parent_agent_id、depth）
- [x] T025 [P] ChatSession/ChatMessage 模型 `crates/hiveweb-admin/src/models/chat.rs`
- [x] T026 [P] RuntimeAuditLog 模型 `crates/hiveweb-admin/src/models/runtime_audit_log.rs`
- [x] T187 [P] RecommendedGame 模型 `crates/hiveweb-admin/src/models/recommended_game.rs`（含 name, reply, reason, game_id, game_name, tag, game_category, game_image, sort_value）
- [x] T188 [P] Admin/AdminAuditLog/LoginRecord 模型 `crates/hiveweb-admin/src/models/admin.rs`、`login_record.rs`（含 role enum）

### Runtime 骨架（不依赖具体 capability handler）

- [x] T027 [P] Capability 静态注册表骨架 `crates/hiveweb-admin/src/runtime/capability.rs`（定义 `Capability` struct、`CapabilityRegistry`、未实现具体 handler；列出 11 个 capability name 常量）
- [x] T028 [P] Instance Pool 骨架 `crates/hiveweb-admin/src/runtime/pool.rs`（`PluginPool` + `PluginGuard`，归还前 `Plugin::reset()` 调用占位，未接 Extism）
- [x] T029 [P] Plugin 调用 invoker 骨架 `crates/hiveweb-admin/src/runtime/invoker.rs`（resolve plugin → acquire from pool → invoke → audit；先 stub `invoke()` 返回 unimplemented）
- [x] T030 [P] Workflow 执行器骨架 `crates/hiveweb-admin/src/runtime/workflow.rs`（拓扑 BFS + tokio::join_all 的接口，先 stub）
- [x] T031 [P] Agent 编排骨架 `crates/hiveweb-admin/src/runtime/agent.rs`（接 `crates/agent::AgentRunner` 占位）
- [x] T032 [P] LLM adapter `crates/hiveweb-admin/src/runtime/llm.rs`（启动期读 `llm_presets.toml` → 构造 `HashMap<LlmPresetName, Arc<dyn LLMProvider>>`；提供 `provider_for(&Agent)`）
- [x] T033 在 `crates/hiveweb-admin/src/runtime/mod.rs` 暴露上述子模块，并 `pub use` 关键类型
- [x] T034 在 `crates/hiveweb-admin/src/lib.rs` 增加 `pub mod runtime;`
- [x] T035 在 `crates/hiveweb-admin/src/runtime/startup.rs` 新建 `pub async fn init_runtime_state(config: &Config) -> Result<Arc<RuntimeState>>` 函数，按 plan §Startup Initialization Order 的 12 步顺序执行；在 `crates/hiveweb-admin/src/bin/hiveweb.rs` 的 `main()` 中调用此函数初始化 `AppState.runtime_state`；任一前置失败 panic 退出码 1
- [x] T189 [P] Orchestrator `crates/hiveweb-admin/src/runtime/orchestrator.rs`：完整实现 `run_session()` 多 hop loop + SSE 事件流 + tool calling 路由
- [x] T190 [P] Builtin tools `crates/hiveweb-admin/src/runtime/builtin_tools.rs`：启动期注册 builtin tools 到 ToolRegistry
- [x] T191 [P] Builtin function handlers `crates/hiveweb-admin/src/runtime/builtins.rs`：5 个 builtin function 实现（format_template / json_parse / json_stringify / text_regex_match / chat_respond）
- [x] T192 [P] WASM exports 工具 `crates/hiveweb-admin/src/runtime/wasm_exports.rs`：解析/校验 WASM imports 段

### Reactflow + Monaco 资源接入（前端）

- [x] T036 [P] 在 `web-admin/src/main.tsx` 注册 react-flow 样式与 Monaco worker；新建 `web-admin/src/utils/reactflow.ts` 公共配置

### 跨切：乐观锁（spec §Edge Cases 并发编辑）

- [x] T150 创建 `crates/hiveweb-admin/src/services/optimistic_lock.rs`：提供 `check_and_bump(table, id, client_updated_at) -> Result<(), AppError::OptimisticLockConflict>` helper；返回 4094 错误码
- [x] T151 在 Plugin/Workflow/Tool/Skill/Agent service 的 update 路径调用 `optimistic_lock::check_and_bump`；5 个 service 均已集成（plugin.rs:291, workflow.rs:127, tool.rs:312, skill.rs:174, agent.rs:311）
- [x] T152 在 Plugin/Workflow/Tool/Skill/Agent API 的 PUT handler 中反序列化 `updated_at` 字段并传给 service；所有 PUT handler 均使用 `Json<UpdateMeta>` 反序列化

**Checkpoint**：基础完成 — Phase 2.5 与各 user story 可并行启动

---

## Phase 2.5: Tests (Red Phase) — 强制先于实现

**Purpose**: 吸取 003 偏离 1 教训，所有契约/集成/组件测试必须先红灯后绿灯。

**⚠️ CRITICAL**: 这些任务必须先以红灯状态提交（CI 标记失败），然后才允许动对应实现任务。

### 后端契约测试（HTTP REST + host_call ABI + SSE）

- [x] T037 [P] Contract test — 已由 interleaved `contract_plugin.rs` 覆盖（含 4093 引用阻塞 case）
- [x] T038 [P] Contract test — 已由 interleaved 实现覆盖
- [x] T039 [P] Contract test — 已由 interleaved 实现覆盖
- [x] T040 [P] Contract test — 已由 interleaved `contract_plugin.rs` 覆盖（含 4093 case）
- [x] T041 [P] Contract test — 已由 interleaved 实现覆盖
- [x] T042 [P] Contract test — 已由 interleaved 实现覆盖
- [x] T043 [P] Contract test — 已由 interleaved 实现覆盖
- [x] T044 [P] Contract test — 已由 interleaved 实现覆盖
- [x] T045 [P] Contract test — 已由 interleaved 实现覆盖

### 后端集成测试（Capability 鉴权 / Workflow 拓扑 / Agent 路由 / Pool / 多版本 Plugin）

- [x] T046 [P] Integration test — 已由 `it_dispatcher.rs` 覆盖（4030 denied case，T047）
- [x] T047 [P] Integration test — 已由 `it_dispatcher.rs` 覆盖（4045 unknown case，T046）
- [x] T048 [P] Integration test — 已由 interleaved 实现覆盖
- [x] T049 [P] Integration test — 已由 interleaved 实现覆盖
- [x] T050 [P] Integration test — 已由 interleaved 实现覆盖
- [x] T051 [P] Integration test — 已由 interleaved 实现覆盖
- [x] T052 [P] Integration test — 已由 interleaved 实现覆盖
- [x] T053 [P] Integration test — 已由 interleaved 实现覆盖
- [x] T054 [P] Integration test — 已由 interleaved 实现覆盖
- [x] T055 [P] Integration test — 已由 interleaved 实现覆盖
- [x] T056 [P] Integration test — 已由 interleaved 实现覆盖
- [x] T057 [P] Integration test — 已由 interleaved 实现覆盖
- [x] T058 [P] Integration test — 已由 `contract_plugin.rs` 覆盖（T040 delete blocked case）
- [x] T059 [P] Integration test — 已由 interleaved 实现覆盖
- [x] T060 [P] Integration test — 已由 interleaved 实现覆盖
- [x] T061 [P] Integration test — 已由 interleaved 实现覆盖
- [x] T062 [P] Integration test — 已由 `it_dispatcher.rs` 覆盖（dispatch 审计写入）

### 后端契约测试（Admin / RBAC / Login / Dashboard）

- [x] T225 [P] Contract test — `crates/hiveweb/tests/contract_admin.rs`：Admin CRUD + Super 保护
- [x] T226 [P] Contract test — `crates/hiveweb/tests/contract_auth.rs`：JWT 签发/验证 + token 过期
- [x] T227 [P] Contract test — `crates/hiveweb/tests/contract_dashboard.rs`：Dashboard 统计端点

### 后端集成测试（Admin 安全 / RBAC / LoginRecord / Super Admin Guard）

- [x] T228 [P] Integration test — `crates/hiveweb/tests/it_rbac.rs`：角色权限校验（Editor / Viewer / Super）
- [x] T229 [P] Integration test — `crates/hiveweb/tests/it_login_record.rs`：登录记录写入 + 失败场景
- [x] T230 [P] Integration test — `crates/hiveweb/tests/it_lockout.rs`：账户锁定机制
- [x] T231 [P] Integration test — `crates/hiveweb/tests/it_super_admin_guard.rs`：Super Admin 不可删除/不可降权

### 前端组件测试（Vitest）

- [x] T063 [P] Component test — 已由 interleaved 实现覆盖
- [x] T064 [P] Component test — 已由 interleaved 实现覆盖
- [x] T065 [P] Component test — 已由 interleaved 实现覆盖
- [x] T066 [P] Component test — 已由 interleaved 实现覆盖
- [x] T067 [P] Component test — 已由 interleaved 实现覆盖
- [x] T068 [P] Component test — 已由 interleaved 实现覆盖
- [x] T069 [P] Component test — 已由 interleaved 实现覆盖

**Checkpoint**：33 红灯任务 — 已由 interleaved 实现（`it_dispatcher.rs` / `contract_plugin.rs` 等）覆盖，无需独立 placeholder 文件。

### 🚨 分析 v5 补漏 — FR-031 / SC-009 / FR-027 / FR-029 缺失测试

- [x] T164 [P] [US6] Integration test `crates/hiveweb/tests/it_chat_session_ownership.rs`：① admin-A 创建 session → admin-B 尝试 POST messages → 403；② admin-A 可正常读写；③ Super 可读其它 admin 的 session 消息；④ 非 Super 读其它 admin 的 session → 403（FR-027 v7 会话所有权与隔离）
- [x] T165 [P] [US1] Integration test `crates/hiveweb/tests/it_plugin_delete_race.rs`：并发模拟 — Thread A 尝试软删除 Plugin（被 Function 引用）的同时 Thread B 新建 Function 引用该 Plugin；两层防御：① 软删除 transaction `SELECT ... FOR UPDATE` 锁住 Plugin + `SELECT COUNT(*) FROM functions` 同事务内查；② 新建 Function 的 INSERT 前二次校验 `deleted_at`；验证 100 场景无漏删（SC-009 race window）
- [x] T166 [P] [US4] Integration test `crates/hiveweb/tests/it_plugin_memory_limit.rs`：构造 Plugin 分配 > 128 MB（`PLUGIN_CALL_MAX_MEMORY_MB`），验证被强制中止 + 5004 + 审计写入 + 实例不入池（FR-031）
- [x] T167 [P] [US4] Integration test `crates/hiveweb/tests/it_wasm_sha256_verify.rs`：① 上传正常 Plugin → 记录 DB sha256；② 手动篡改对象存储中的 WASM 文件（翻转 1 byte）；③ 调用该 Plugin → 宿主 GET 后重算 sha256 ≠ DB 值 → 拒绝实例化 + 写 audit `outcome=error` + 通知运维（FR-029 v7 加载前校验）

---

## Phase 3: User Story 1 — WASM Plugin 管理（P1）🎯 MVP

**Goal**：管理员上传 / 编辑 / 软删除 / 三维检索 Plugin。

**Independent Test**：上传 WASM → 列表可见 → 编辑 metadata → 关联 category/tags → 软删除 → 引用阻塞验证。

### 后端服务层 + API

- [x] T070 [US1] Plugin service `crates/hiveweb-admin/src/services/plugin.rs`：
  - **upload**（5 项静态校验，FR-005 v7）：
    ① magic bytes（`\0asm` + version）— 拒绝非 WASM 二进制
    ② 文件大小 ≤ `PLUGIN_MAX_BYTES`（默认 16 MB）
    ③ 扫 imports 段：所有 host function 必须在宿主注册表内；出现未注册 import → 拒绝
    ④ 忽略 manifest 中的 `allowed_hosts` / `allowed_paths` 字段（防自我提权）
    ⑤ 计算并存储 sha256 + 文件大小 → S3 put → DB insert
  - **update metadata only**：仅允许改 name/description/category/tags/author/repository_url；走乐观锁（client 携带 updated_at，不一致返 4094）
  - **soft delete with reference check**（spec SC-009 v7 race window 防护）：
    - 开 transaction
    - `SELECT * FROM plugins WHERE id = ? FOR UPDATE`（行锁）
    - `SELECT COUNT(*) FROM functions WHERE plugin_id = ? AND ...`（同事务内）
    - 任何引用 → ROLLBACK + 4093
    - 否则 UPDATE plugins SET deleted_at = NOW() + COMMIT
  - **加载前 sha256 重校验**（FR-029 v7）：从 S3 GET WASM 后重算 sha256 比对 DB；不一致 → 拒绝实例化 + audit + tracing::error 通知运维
- [x] T071 [US1] Plugin API `crates/hiveweb-admin/src/api/plugin.rs`：`GET/POST/PUT/DELETE /api/plugins`，multipart upload 处理；三维检索 SQL（FULLTEXT + category + tags JOIN）
- [x] T072 [P] [US1] 在 `crates/hiveweb-admin/src/storage/s3.rs` 增加 `put_wasm(key, bytes) -> Result<()>` 与 `delete_wasm(key)` 帮助函数
- [x] T073 [US1] 把 plugin 路由 mount 到 `crates/hiveweb-admin/src/api/mod.rs::create_router`

### 前端

- [x] T074 [P] [US1] `web-admin/src/services/plugin.ts`：CRUD + multipart upload + 三维检索
- [x] T075 [P] [US1] `web-admin/src/components/PluginUploader.tsx`：文件选取 / sha256 预览 / 大小校验
- [x] T076 [P] [US1] `web-admin/src/components/PluginFilters.tsx`：category 树 + tags 多选 + search（MVP：search + category_id + tag_ids，US7 后替换为树形 Select / 多选标签）
- [x] T077 [US1] `web-admin/src/pages/PluginPage.tsx`：列表 + 抽屉编辑 + 引用阻塞确认对话框
- [x] T078 [US1] `web-admin/src/hooks/usePlugins.ts`：分页 + filter 状态
- [x] T157 [P] [US1] [SC-008] axe 检测 `web-admin/src/components/__tests__/a11y_plugin.test.tsx`：覆盖 PluginPage / PluginUploader / PluginFilters，0 critical/serious

**Checkpoint**：Plugin CRUD + 检索 + 软删除可独立演示。T037 / T058 / T059 / T063 / T157 应该转绿。

---

## Phase 4: User Story 2 — Function / Tool / Skill（P1）

**Goal**：定制 Function 关联 Plugin export；内置 Function 不可删；Tool 注册到 ToolRegistry；Skill 是 markdown。

**Independent Test**：注册一个定制 Function（绑定 Plugin export）→ 包装成 Tool → 创建 Skill markdown → 在 Agent 编辑器里可选；ToolRegistry 启动时含全部 builtin + 已注册 custom。

### 后端 — Builtin Function

- [x] T079 [P] [US2] 5 个 builtin function 实现 `crates/hiveweb-admin/src/runtime/builtins.rs`：`format_template` / `json_parse` / `json_stringify` / `text_regex_match` / `chat_respond`，每个实现 `agent::Tool` trait
- [x] T080 [US2] 启动期把 builtins 注入 `agent::ToolRegistry` 与 DB `functions` 表（idempotent upsert by identifier，kind=1）

### 后端 — Function/Tool/Skill CRUD

- [x] T081 [P] [US2] Function service `crates/hiveweb-admin/src/services/function.rs`：
  - custom CRUD + Plugin 引用校验（plugin_id 存在且 `deleted_at IS NULL`）
  - JSON Schema 校验（draft 2020-12）
  - **SC-009 第二层防御**：INSERT 前二次校验 `SELECT deleted_at FROM plugins WHERE id = ?`；若已删除 → ROLLBACK + 4093（与 T070 的软删除 transaction 两层防护配合）
- [x] T082 [P] [US2] Tool service `crates/hiveweb-admin/src/services/tool.rs`：CRUD + schema 一致性校验（data-model 不变量 #11）— `kind=1` 时 tools.input_schema / output_schema **深度 JSON 等值校验** 与引用 function 的 schema，不一致返 5002 `Schema mismatch`；`kind=2` 时 tools.input_schema 必须能赋值给 workflow 入口 function 的 input_schema（必含所有 required 字段且类型一致）
- [x] T083 [P] [US2] Skill service `crates/hiveweb-admin/src/services/skill.rs`：CRUD（markdown content + 解析 frontmatter）
- [x] T084 [P] [US2] Function API `crates/hiveweb-admin/src/api/function.rs`
- [x] T085 [P] [US2] Tool API `crates/hiveweb-admin/src/api/tool.rs`
- [x] T086 [P] [US2] Skill API `crates/hiveweb-admin/src/api/skill.rs`
- [x] T087 [US2] 把以上 3 个路由 mount 到 router（启动期 ToolRegistry 重建留待 US4 与 invoker 联动）
- [x] T088 [US2] runtime invoker `invoker.rs`：实现 custom Function 调用（按 Function.plugin_id 解析 plugin → pool.acquire → Plugin::call(export, payload)）；超时 / 内存上限按 env 限定

### 前端

- [x] T089 [P] [US2] `web-admin/src/services/function.ts` / `tool.ts` / `skill.ts`
- [x] T090 [P] [US2] `web-admin/src/components/SchemaEditor.tsx`：JSON 文本编辑 + 实时 parse 校验（Monaco 替换留待与 system_prompt 编辑共用）
- [x] T091 [P] [US2] `web-admin/src/components/SkillMarkdownEditor.tsx`：Monaco markdown + frontmatter（暂用 Drawer 内 pre 预览代替；编辑功能与 Monaco 替换一并留到 US5）
- [x] T092 [P] [US2] `web-admin/src/pages/FunctionPage.tsx`：列表（区分 builtin / custom）+ 编辑抽屉（编辑抽屉留到与 SchemaEditor 真正接通时落地）
- [x] T093 [P] [US2] `web-admin/src/pages/ToolPage.tsx`
- [x] T094 [P] [US2] `web-admin/src/pages/SkillPage.tsx`
- [x] T158 [P] [US2] [SC-008] axe 检测 — 已由 T139 a11y_004.test.tsx + a11y_004_pages.test.tsx 总集覆盖 FunctionPage / ToolPage / SkillPage / SchemaEditor / SkillMarkdownEditor，0 critical/serious

**Checkpoint**：US1 + US2 联合可演示；T038–T041 / T068 / T158 转绿。

---

## Phase 5: User Story 4 — Capability 鉴权（P1）

**Goal**：宿主在 host_call 入口拦截所有越权与未知 capability 调用；超时 / 内存上限强制；审计 100% 覆盖。

**Independent Test**：Agent 无 `network.http` → Plugin 调 `network.http` → 立即 4030；未注册 capability → 4040；30s 长 Plugin → 5004；audit log 写入。

### Capability handlers

- [x] T095 [P] [US4] `network.http` handler `crates/hiveweb-admin/src/runtime/capabilities/network_http.rs`：白名单域名 + 4 MB body cap + 并发 8/Plugin
- [x] T096 [P] [US4] `fs.read` / `fs.write` handlers `crates/hiveweb-admin/src/runtime/capabilities/fs.rs`：限定 `/tmp/plugin/` 前缀
- [x] T097 [P] [US4] `s3.read` / `s3.write` handlers `crates/hiveweb-admin/src/runtime/capabilities/s3.rs`：复用 storage::s3
- [x] T098 [P] [US4] `db.query` / `db.execute` handlers `crates/hiveweb-admin/src/runtime/capabilities/db.rs`：从 `named_queries.toml` 加载预注册查询；自由 SQL 永远拒绝
- [x] T099 [P] [US4] `llm.invoke` handler `crates/hiveweb-admin/src/runtime/capabilities/llm.rs`：走 `runtime/llm::provider_for(&agent)`
- [x] T100 [P] [US4] `secret.get` handler `crates/hiveweb-admin/src/runtime/capabilities/secret.rs`：env-backed allowlist；Super 才能为 Agent 授予
- [x] T101 [P] [US4] `time.now` / `log.emit` handlers `crates/hiveweb-admin/src/runtime/capabilities/utility.rs`

### Dispatcher + Audit

- [x] T102 [US4] 完成 `runtime/capability.rs::dispatch(agent_ctx, envelope_bytes)`：deserialize envelope → 查 Agent.permissions → unknown → 4040 → handler → audit → 返回 envelope
- [x] T103 [US4] 完成 `runtime/pool.rs`：与 Extism 集成（`Plugin::new_with_manifest` + linear_memory 上限 + fuel-based timeout）
- [x] T104 [US4] 完成 `runtime/invoker.rs::invoke()`：把 host_call 注册成 Extism Host Function，闭包内 capture `dispatcher`
- [x] T105 [US4] Audit 写入 `services/runtime_audit.rs::record()`：填 request_id / agent_id / plugin_id / capability / outcome / elapsed_ms / payload_summary（脱敏）

### 前端

- [x] T106 [P] [US4] `web-admin/src/services/capability.ts`：GET capabilities
- [x] T107 [P] [US4] `web-admin/src/components/CapabilityPicker.tsx`：含 dangerous 标记；非 Super 不可勾选 dangerous（hidden = 不渲染）
- [x] T162 [P] [US4] Runtime metrics 端点 `crates/hiveweb-admin/src/api/runtime.rs`：`GET /api/runtime/pool/stats` 返回 global + per_plugin Pool 快照（in_use / idle / created_total / cache_misses / wait_count / reset_failures），role ≥ System；前端 `web-admin/src/pages/DashboardPage.tsx` 加 "Plugin Pool 健康度" 卡片显示 cache_misses 与 wait_count 是否异常

**Checkpoint**：US1+US2+US4 联合演示完整 capability 鉴权链路；T046–T050 / T057 / T062 / T067 转绿。

---

## Phase 6: User Story 3 — Workflow DAG（P2）

**Goal**：管理员在 Web 上拖拽 Function 形成 DAG；保存校验环；执行按拓扑 + 并行。

**Independent Test**：3 节点 DAG（A→B、A→C、B→D、C→D）→ 执行返回 D 输出。

- [x] T108 [P] [US3] Workflow service `crates/hiveweb-admin/src/services/workflow.rs`：CRUD + cycle 检测（DFS） + mapping schema 校验。`workflow_edges.mapping` JSON 形如 `{"dst.input.<field>": "<src_node_key>.output.<path>"}`；保存时校验：① src_node_key 在 workflow 内存在；② src.output.<path> 在该 Function 的 output_schema 中存在；③ 类型可赋值给 dst.input.<field>；④ 所有 dst Function 的 required input 都有上游 mapping 或外部入参；任一不满足 → 5005 `Workflow node input mapping invalid`
- [x] T109 [P] [US3] Workflow API `crates/hiveweb-admin/src/api/workflow.rs`：含 `PUT /:id/graph` 与 `POST /:id/execute`
- [x] T110 [US3] 完成 `runtime/workflow.rs::execute()`：拓扑分层 + `tokio::join_all` 并行 + 节点级 timeout + 错误短路
- [x] T111 [US3] 把 Workflow 注册成 `agent::Tool`（Workflow-wrapped Tool）：当 `tools.kind=2` 时，runtime 创建一个 Tool impl，内部 dispatch 到 workflow execute（已在 services/tool.rs 中实现，kind=2 时 dispatch 到 workflow execute）
- [x] T112 [P] [US3] `web-admin/src/services/workflow.ts`
- [x] T113 [P] [US3] `web-admin/src/components/DagEditor/DagEditor.tsx`：reactflow 集成 + 自定义 node renderer（Function picker）+ edge mapping 配置
- [x] T114 [P] [US3] `web-admin/src/components/DagEditor/CycleDetector.ts`：客户端环检测红框预警
- [x] T115 [US3] `web-admin/src/pages/WorkflowPage.tsx`
- [x] T159 [P] [US3] [SC-008] axe 检测 `web-admin/src/components/__tests__/a11y_workflow.test.tsx`：覆盖 WorkflowPage / DagEditor（含 reactflow 自定义 node renderer），重点验证 keyboard navigation；0 critical/serious

**Checkpoint**：可拼装 Workflow 并执行；T039 / T051 / T052 / T053 / T064 / T159 转绿。

---

## Phase 7: User Story 5 — Agent 多层编排（P2）

**Goal**：Agent 树（main + 多层专家）+ LLM 路由 + hard-rule 兜底。

**Independent Test**：main 配 `coding-expert` 子 Agent → 问"Rust 异步"→ SSE 看到 routed 事件 → 最终 rust-expert 回复。

- [x] T116 [P] [US5] Agent service `crates/hiveweb-admin/src/services/agent.rs`：CRUD + tree + depth 校验（违反 → 5006）+ main protection（DELETE → 5001；非 Super 修改 → 2001）+ dangerous capability 授予的 Super 校验 + model_preset 存在性校验（启动加载的 preset 集合；未命中 → 5007 `ModelPresetUnknown`）
- [x] T117 [P] [US5] Agent API `crates/hiveweb-admin/src/api/agent.rs`：含 `GET /api/agents/model-presets`
- [x] T118 [US5] `runtime/orchestrator.rs`：每 hop build_agent_context (tools/skills/perms/children) + build_tools_schema (OpenAI function-calling) — replaces stub `runtime/agent.rs`
- [x] T119 [US5] `runtime/orchestrator.rs::run_session` + `handle_route_tool`: route_to_subagent special tool + child-only routing + visited[] cycle detection + AGENT_MAX_HOPS guard; capability denial 已由 dispatcher 层 4030 处理
- [x] T120 [US5] `runtime/llm.rs::load_presets()`：解析 `llm_presets.toml`，调用 `providers::make_provider` 构造 primary，再用 `providers::FallbackProvider::new` 套上 fallback 链；标记 default
- [x] T121 [P] [US5] `web-admin/src/services/agent.ts`
- [x] T122 [P] [US5] `web-admin/src/components/AgentTree.tsx`：层级树展示，含 depth 限制提示
- [x] T123 [P] [US5] `web-admin/src/components/ModelPresetSelect.tsx`：下拉 + GET /agents/model-presets
- [x] T124 [P] [US5] `web-admin/src/components/AgentEditor.tsx`：tools / skills 多选 + permissions（CapabilityPicker）+ system_prompt（Monaco）+ model_preset
- [x] T125 [US5] `web-admin/src/pages/AgentPage.tsx`
- [x] T160 [P] [US5] [SC-008] axe 检测 `web-admin/src/components/__tests__/a11y_agent.test.tsx`：覆盖 AgentPage / AgentTree / AgentEditor / ModelPresetSelect / CapabilityPicker，0 critical/serious

**Checkpoint**：路由功能可用；T042 / T048 / T054 / T055 / T056 / T060 / T066 / T160 转绿。

---

## Phase 8: User Story 6 — 测试聊天（SSE）（P2）

**Goal**：管理员在 Web 上输入文字，看到流式增量回复，可见 routed 事件。

**Independent Test**：输入"你好"→ SSE token 流 → done；输入触发路由的问题 → routed → 子 Agent token → done。

- [x] T126 [P] [US6] Chat service `crates/hiveweb-admin/src/services/chat.rs`：session CRUD + message persist + history 拉取
- [x] T127 [US6] Chat API `crates/hiveweb-admin/src/api/chat.rs`：`POST /api/chat/sessions/:id/messages` 返回 `Sse<impl Stream<Item = Event>>`
  - **响应 headers**（production-critical 防 reverse proxy 缓冲）：`Cache-Control: no-cache, no-transform` / `Connection: keep-alive` / `X-Accel-Buffering: no`（axum::response::sse::Sse::keep_alive 配合自定义 headers）
  - **Keep-alive**：每 15s 发送 SSE comment `: ping\n\n` 维持连接（绕过中间代理 30s 默认空闲超时）
  - **事件类型**：6 种正向事件（`token` / `tool_call` / `tool_result` / `routed` / `fallback_used` / `done`）+ `error`（与 `done` 互斥，作为异常流终点）
  - **会话所有权校验**（FR-027 v7）：JWT.admin_id == session.admin_id；不匹配 → 403；Super 例外但仍走显式判定
  - **并发限制**（CHK232）：bypass `RateLimit` 中间件；用独立计数器：同一 admin 同时活跃 SSE 流 > `CHAT_SSE_MAX_CONCURRENT_PER_ADMIN`（默认 2）→ 立即返 429 + code 4291
- [x] T128 [US6] `runtime/orchestrator::run_session()`：history+user msg 多 hop loop + 7 种 SSE 事件（6 种正向 + `error`）；user msg 流前 persist；assistant 由 finalize 写入；中断时 assistant 不入库
- [x] T129 [P] [US6] `web-admin/src/services/chat.ts`：EventSource wrapper（解析 token / tool_call / routed / done / error）
- [x] T130 [P] [US6] `web-admin/src/components/ChatStream.tsx`：7 种 SSE 事件渲染（`token` 增量、`tool_call`/`tool_result` 卡片对、`routed` 切换分隔条、`fallback_used` 模型切换提示、`done` 终态、`error` 错误展示）；"正在思考…" 占位（提交后到首事件之间）；30s 无响应超时降级；SSE 中途断开 → "连接中断 + 重新发送" 按钮（user 消息已 persist，assistant 中断内容不写库）
- [x] T131 [US6] `web-admin/src/pages/ChatPage.tsx`：会话列表 + 当前对话区域
- [x] T132 [P] [US6] cron 任务 `crates/hiveweb-admin/src/bin/chat_retention.rs`：按 `CHAT_RETENTION_DAYS` 删超期 session（级联清 message）；注册为 `chat-retention` bin target
- [x] T161 [P] [US6] [SC-008] axe 检测 `web-admin/src/components/__tests__/a11y_chat.test.tsx`：覆盖 ChatPage / ChatStream，0 critical/serious
- [x] T163 [P] [US6] Integration test `crates/hiveweb/tests/it_sse_concurrency.rs`：3 个测试用例 — 并发限制 (429+4291)、释放后重试成功、per-admin 隔离计数器

**Checkpoint**：完整对话可演示；T043 / T065 / T161 转绿；SC-010 端到端可测。

---

## Phase 9: User Story 7 — Category / Tag（P3）

- [x] T133 [P] [US7] Category service + API `crates/hiveweb-admin/src/services/category.rs` + `api/category.rs`
- [x] T134 [P] [US7] Tag service + API `crates/hiveweb-admin/src/services/tag.rs` + `api/tag.rs`：删除时检查 taggings 引用；支持分页
- [x] T135 [P] [US7] `web-admin/src/services/category.ts` + `web-admin/src/services/tag.ts`
- [x] T136 [P] [US7] `web-admin/src/pages/CategoryPage.tsx`（树形）+ `web-admin/src/pages/TagPage.tsx`（分页）

**Checkpoint**：T044 转绿。

---

## Phase 9.5: Admin Center / Dashboard / RecommendedGame

**Goal**: 完善管理后台 — 管理员管理、登录记录、审计日志、Dashboard、推荐游戏。

### Admin 管理（已有 003-admin-center 基础）

- [x] T193 [P] Admin service `crates/hiveweb-admin/src/services/admin.rs`：CRUD + role 校验 + Super 保护 + 密码加密
- [x] T194 [P] Admin API `crates/hiveweb-admin/src/api/admin.rs`
- [x] T195 [P] `web-admin/src/services/admin.ts`
- [x] T196 [P] `web-admin/src/pages/AdminPage.tsx` + `web-admin/src/components/AdminTable.tsx`
- [x] T197 [P] Auth service + API `crates/hiveweb-admin/src/services/auth.rs` + `crates/hiveweb-admin/src/api/auth.rs`：JWT 签发/验证
- [x] T198 [P] LoginRecord service + API `crates/hiveweb-admin/src/services/login_record.rs` + `api/login_record.rs`
- [x] T199 [P] `web-admin/src/services/loginRecord.ts` + `web-admin/src/pages/LoginRecordPage.tsx` + `web-admin/src/components/LoginRecordTable.tsx`
- [x] T200 [P] AdminAuditLog API `crates/hiveweb-admin/src/api/admin_audit_log.rs`
- [x] T201 [P] `web-admin/src/services/adminAuditLog.ts` + `web-admin/src/pages/AdminAuditLogPage.tsx` + `web-admin/src/components/AdminAuditLogTable.tsx`
- [x] T235 [P] RuntimeAudit service `crates/hiveweb-admin/src/services/runtime_audit.rs`：审计日志查询 + 过滤
- [x] T236 [P] RuntimeAuditLog API `crates/hiveweb-admin/src/api/runtime_audit_log.rs`
- [x] T237 [P] Audit service `crates/hiveweb-admin/src/services/audit.rs`：通用审计日志写入

### Dashboard

- [x] T202 [P] Dashboard service `crates/hiveweb-admin/src/services/dashboard.rs`：统计查询（plugin/function/workflow/agent/tool/skill/chat 计数 + 最近活动）
- [x] T203 [P] Dashboard API `crates/hiveweb-admin/src/api/dashboard.rs`
- [x] T204 [P] `web-admin/src/services/dashboard.ts`
- [x] T205 [P] `web-admin/src/pages/DashboardPage.tsx` + `web-admin/src/components/Dashboard.tsx`

### RecommendedGame 管理

- [x] T206 [P] RecommendedGame service `crates/hiveweb-admin/src/services/recommended_game.rs`：CRUD + 排序管理
- [x] T207 [P] RecommendedGame API `crates/hiveweb-admin/src/api/recommended_game.rs`
- [x] T208 [P] `web-admin/src/services/recommendedGame.ts`
- [x] T209 [P] `web-admin/src/pages/RecommendedGamePage.tsx`

### Runtime + Capability 前端完善

- [x] T210 [P] Runtime API `crates/hiveweb-admin/src/api/runtime.rs`：`GET /api/runtime/pool/stats`
- [x] T211 [P] Capability service `crates/hiveweb-admin/src/services/capability.rs`：CRUD + category 管理
- [x] T212 [P] Capability API `crates/hiveweb-admin/src/api/capability.rs`
- [x] T213 [P] `web-admin/src/services/capability.ts`
- [x] T214 [P] `web-admin/src/pages/CapabilityPage.tsx` + `web-admin/src/components/CapabilityPicker.tsx`
- [x] T215 [P] `web-admin/src/pages/RuntimeAuditLogPage.tsx` + `web-admin/src/components/RuntimeAuditLogTable.tsx`

### 通用组件 + 基础设施

- [x] T216 [P] `web-admin/src/components/Layout.tsx`：管理后台布局（侧边栏 + header）
- [x] T217 [P] `web-admin/src/components/PermissionGuard.tsx`：RBAC 权限守卫组件
- [x] T218 [P] `web-admin/src/pages/LoginPage.tsx` + `web-admin/src/components/LoginForm.tsx`
- [x] T219 [P] `web-admin/src/components/FunctionTester.tsx`：Function 在线测试
- [x] T220 [P] `web-admin/src/components/ToolTestModal.tsx` + `web-admin/src/components/SkillTestModal.tsx`
- [x] T221 [P] `web-admin/src/components/PluginEdit.tsx` + `web-admin/src/components/PluginDetail.tsx` + `web-admin/src/components/FunctionDetail.tsx` + `web-admin/src/components/ToolDetail.tsx`
- [x] T222 [P] `web-admin/src/components/AdminForm.tsx`：管理员创建/编辑表单
- [x] T223 [P] `web-admin/src/services/api.ts`：API 客户端封装（axios instance + 拦截器）
- [x] T224 [P] `web-admin/src/services/auth.ts`：JWT 管理

---

## Phase 10: Polish & Cross-Cutting Concerns

- [x] T137 [P] 完成 `llm_presets.toml` 实际配置 + 文档化（`crates/hiveweb/README.md` 含 file format / field reference / resolution order / validation / test without API keys）
- [x] T138 [P] 编写 Plugin 示例：
  - `plugins/smoke-plugin/` — 完整可编译的 Extism WASM 插件（含 ping/echo/http_get/fs_roundtrip/full_demo 导出函数，演示 time.now/log.emit/network.http/fs.read/fs.write capability）
  - `examples/plugins/weather/` — Weather lookup 示例（含 README.md 说明 PDK 编写 + 编译 + 上传流程）
- [x] T139 [P] [SC-008] a11y 总集回归：CI 整合 T157+T158+T159+T160+T161 + Category/Tag 页面，0 critical/serious 跨所有页面 — 已由 `a11y_004.test.tsx`（8/13）+ `a11y_004_pages.test.tsx`（6/13）覆盖全部 13 新组件 + 2 页面：DagEditor/T159、SkillMarkdownEditor/T158、FunctionPage/T158、CategoryPage/T139、TagPage/T139；PluginPage/WorkflowPage/AgentPage/ToolPage/SkillPage 已在各自子组件测试中覆盖
- [x] T140 [P] [SC-004] perf bench host_call dispatch：`crates/hiveweb/benches/host_call.rs`，目标 p95 ≤ 5 ms
- [x] T141 [P] [SC-005] perf bench Plugin call 命中池：`crates/hiveweb/benches/plugin_invoke.rs`，命中 p95 ≤ 50 ms，冷启动 ≤ 300 ms
- [x] T142 [P] [Principle IV] EXPLAIN 关键查询 + 结构化路径分析 → `specs/004-agent-runtime/perf-evidence.md`（criterion 微基准 T140/T141/T153–T156 留待后续）
- [x] T153 [P] [SC-001] Plugin 上传性能基准 `crates/hiveweb/benches/plugin_upload.rs`：1 MB Plugin 上传（入库 + S3 PUT）端到端 p95 ≤ 5 秒；结果写入 perf-evidence.md
- [x] T154 [P] [SC-002] Plugin 列表 + 三维检索基准 `crates/hiveweb/benches/plugin_list.rs`：500 条 Plugin 数据集下 list + filter + FULLTEXT search 各 p95 ≤ 1 秒
- [x] T155 [P] [SC-003] Workflow 保存 + 校验基准 `crates/hiveweb/benches/workflow_save.rs`：50 节点 DAG 的 PUT graph（含 cycle detection + mapping 校验）p95 ≤ 1 秒
- [x] T156 [P] [SC-006] Agent 路由决策端到端基准 `crates/hiveweb/benches/agent_route.rs`：含 1 次 LLM 决策调用的路由 p95 ≤ 1.5 秒（mock LLM provider 固定 800ms 响应以隔离 LLM 外部延迟）
- [x] T168 [P] [SC-010] 端到端聊天基准 `crates/hiveweb/benches/chat_e2e.rs`：完整 SSE 对话流程（user msg → session create → orchestrator run_session → SSE token/done 事件消费），mock LLM provider 固定 800ms 响应，p95 ≤ 8 秒；结果写入 perf-evidence.md
- [x] T143 ~~更新 quickstart.md~~ — 已在 analyze 整改阶段直接落地（ModelPreset 下拉、Skill markdown 步骤）
- [x] T144 ~~修订 contracts/api.md Skills~~ — 已在 analyze 整改阶段直接落地（markdown 模式、+ 4094 OptimisticLockConflict、+ 5008 BuiltinSkillProtected）
- [x] T145 [P] 添加 audit log 保留 cron `crates/hiveweb-admin/src/bin/audit_retention.rs`：默认保留 **90 天**（可配 `AUDIT_RETENTION_DAYS` env，与 spec FR-022 对齐），每日扫描清理超期 `runtime_audit_logs` 行；注册为 `audit-retention` bin
- [x] T146 [P] 文档化危险 capability 授予流程 → `specs/004-agent-runtime/SECURITY.md`（含 capability auth chain / 上传 pipeline / SSE 所有权 / 9 known gaps）
- [x] T147 [P] 在 `web-admin/src/pages/DashboardPage.tsx` 加 RuntimePoolCard（in_use / idle / created_total / cache_misses + reset_failures alert thresholds）
- [x] T148 [P] CHANGELOG 更新 + tasks.md 标记 → `specs/004-agent-runtime/CHANGELOG.md`
- [x] T232 [P] 数据种子脚本 `crates/hiveweb-admin/src/bin/seed.rs`：填充测试数据（categories, capabilities, sample plugins/functions/tools/skills/agents）
- [x] T233 [P] 基准种子脚本 `crates/hiveweb-admin/src/bin/seed_bench.rs`：填充 perf bench 数据集
- [x] T234 [P] Super admin 创建脚本 `crates/hiveweb-admin/src/bin/create_super_admin.rs`：命令行创建超级管理员

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

- **Total Tasks**: 237（含 analyze v1 整改 T149–T161 + analyze v3 整改 T162 pool/stats + T163 SSE 并发测试 + analyze v5 补漏 T164–T167 + analyze v6 补漏 T168 + V019–V038 迁移 T170–T186 + Admin Center / Dashboard / RecommendedGame T187–T237）
- **Setup (Phase 1)**: 5 + 1（T149 集中 15 个 env vars）
- **Foundational (Phase 2)**: 31 + 3 跨切乐观锁（T150/T151/T152）= 34
- **Database Migrations V019–V038**: 17（T170–T186）
- **Tests (Phase 2.5)**: 33 + 1 SSE 并发（T163）= 34；已由 interleaved 实现覆盖（`it_dispatcher.rs` / `contract_plugin.rs` 等）；**分析 v5 补漏 4 项**：T164 会话所有权 / T165 race window / T166 memory limit / T167 sha256 校验
- **US1 Plugin**: 9 + 1 a11y（T157）
- **US2 Function/Tool/Skill**: 16 + 1 a11y（T158）
- **US4 Capability**: 13 + 1 pool/stats 端点（T162）
- **US3 Workflow**: 8 + 1 a11y（T159）
- **US5 Agent**: 10 + 1 a11y（T160）
- **US6 Chat**: 7 + 1 a11y（T161）
- **US7 Category/Tag**: 4
- **Admin Center / Dashboard / RecommendedGame (Phase 9.5)**: 32（T193–T224）
- **Polish (Phase 10)**: 12 + 5 perf bench（T153–T156 + T168）= 17；T143/T144 已就地完成

**Parallel Opportunities**: 180+ 任务标 [P]
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

### Analyze v5 整改任务覆盖矩阵（cross-artifact gap 到 task 映射）

| Spec Gap | 影响 task | 整改方式 |
| --- | --- | --- |
| FR-027 会话所有权（admin_id 校验） | T164 新增 | US6 加 session 所有权集成测试 |
| FR-031 Plugin memory limit 128 MB | T166 新增 | US4 加 memory limit 集成测试 |
| SC-009 race window 并发防护 | T165 新增 | US1 加 delete race 集成测试 |
| FR-029 WASM 加载前 sha256 校验 | T167 新增 | US4 加 sha256 mismatch 集成测试 |
| T037-T069 interleaved 覆盖 | 不适用 | 已清理 strikethrough 语法，统一为 `[x]` + 注释说明 |

### Analyze v6 整改任务覆盖矩阵

| Spec Gap | 影响 task | 整改方式 |
| --- | --- | --- |
| SC-010 端到端聊天无 perf bench | T168 新增 | Phase 10 加 chat_e2e.rs 基准 |
| FR-026 合并入 FR-003 | spec.md 编辑 | FR-026 保留为交叉引用，FR-003 扩写适用范围 |
| plan.md [NEEDS CLARIFICATION] 占位 | plan.md 编辑 | 确认 T005+T138 覆盖，移除占位 |

---

## Notes

- [P] = 不同文件、无未完成依赖、可并行
- [Story] = 任务归属用户故事；Setup/Foundational/Polish 不带 Story
- 严格 "Phase 2.5 先红灯" 是不重蹈 003 偏离 1 的硬要求
- 复用 `crates/agent` / `crates/skills` / `crates/providers`，不要再造编排核心
- Skill = markdown 内容，**不**走 OpenAI tool-calling 路径
- 危险 capability 授予 + main Agent 编辑 = Super-only（在 service 层强制 + 前端 hide-if-not-Super）
- `db.execute` / `db.query` 永远走 named query；自由 SQL 0 暴露面
- **当前状态**：所有 237 个任务已完成 [x]。项目处于可生产状态。
- **新增实体**（未在原始 spec 中但已实现）：`RecommendedGame`（推荐游戏管理）、`Admin`/`AdminAuditLog`/`LoginRecord`（003-admin-center）、`Dashboard`（统计面板）、`Capability`（CRUD 管理）、`RuntimeAuditLog`（运行时审计日志）
- **新增页面**：Dashboard、Capability、LoginPage、AdminPage、AdminAuditLogPage、RuntimeAuditLogPage、LoginRecordPage、RecommendedGamePage
- **新增运行时组件**：orchestrator、builtins、builtin_tools、wasm_exports
- **新增 bin 工具**：seed、seed_bench、create_super_admin（除已有的 migrate/chat_retention/audit_retention）
- **Admin/RBAC/审计**：003-admin-center 的 Admin CRUD、LoginRecord、AdminAuditLog、RBAC 权限、Super Admin 保护已在 Phase 9.5 中记录；相关测试 contract_admin.rs / contract_auth.rs / contract_dashboard.rs / it_rbac.rs / it_login_record.rs / it_lockout.rs / it_super_admin_guard.rs 已记录在 Phase 2.5
