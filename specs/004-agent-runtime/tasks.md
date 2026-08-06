---

description: "Task list for Agent Runtime (Capability-based WASM plugin runtime)"
---

# Tasks: Agent Runtime（Capability-based WASM Plugin Runtime）

> **范围更新（2026-07-23）：**
> - `RecommendedGame`、`recommended_games*` 及 `/api/recommended-games*` 相关管理和公开接口已废弃，仅兼容保留；不得新增调用或扩展。
> - US6 管理端测试聊天及其 admin session/SSE API、UI、表和测试已由 `81a84fe` 移除，属于 **Legacy / Superseded** 历史任务；已勾选仅表示当时完成，不表示当前必须存在。不得恢复 `/api/admin-chat*` 或 `/api/chat/sessions*`。
> - 现行普通用户聊天由 `web-user`、`/api/assistant`、`/api/newsession`、`/api/messages` 和 `chat_*_user` 表承载，属于后续外部 Assistant API 范围。

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
  - US6 (P2)        — 管理端测试聊天（SSE，历史，已 superseded）
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
  - **Plugin 上传 & 调用**：`PLUGIN_MAX_BYTES=16777216`、`PLUGIN_CALL_TIMEOUT_MS=30000`、`PLUGIN_CALL_MAX_MEMORY_MB=128`、`PLUGIN_CALL_FUEL=10000000000`
  - **Instance Pool**：`PLUGIN_POOL_MAX_PER_PLUGIN=8`、`PLUGIN_POOL_MAX_TOTAL=64`、`PLUGIN_POOL_IDLE_TIMEOUT_SEC=600`、`PLUGIN_POOL_ACQUIRE_TIMEOUT_MS=5000`
  - **Agent**：`AGENT_MAX_HOPS=5`；原 `CHAT_RETENTION_DAYS=30` / `CHAT_SSE_MAX_CONCURRENT_PER_ADMIN=2` 属 admin Chat 历史设计，已 superseded
  - **LLM / Audit**：`LLM_PRESETS_PATH=./llm_presets.toml`、`LLM_NODE_TIMEOUT_MS=25000`、`LLM_CHAIN_TIMEOUT_MS=45000`、`AUDIT_RETENTION_DAYS=36500`（100 年默认，可调整）
  DEPLOYMENT.md 同步更新；每个 env 加注释指向对应 FR / research 段。

---

## Phase 2: Foundational（阻塞所有 user story）

**Purpose**: 建表 + runtime 骨架 + 工厂。所有 user story 实现都依赖此。

**⚠️ CRITICAL**: 此阶段完成前不能动 user story 实现，但 Phase 2.5 红灯测试可与本阶段并行起草。

### 数据库迁移（实际物理链 V001–V033；V013 保留空号）

- [x] T006 [P] 核对 V001–V004 管理员、登录与管理审计迁移；以 `crates/hiveweb/migrations/` 中同名 SQL 为准
- [x] T007 [P] 创建 V005 categories 表 `crates/hiveweb/migrations/V005__categories.sql`
- [x] T008 [P] 创建 V006 capabilities 表 `crates/hiveweb/migrations/V006__capabilities.sql`
- [x] T009 [P] 创建 V007 tags + taggings 表 `crates/hiveweb/migrations/V007__tags.sql`
- [x] T010 [P] 创建 V008 plugins 表 `crates/hiveweb/migrations/V008__plugins.sql`
- [x] T011 [P] 创建 V009 functions 表 `crates/hiveweb/migrations/V009__functions.sql`
- [x] T012 [P] 创建 V010 workflows + nodes + edges 表 `crates/hiveweb/migrations/V010__workflows.sql`
- [x] T013 [P] 创建 V011 tools + skills 与 V012 agents/ACL 表；对应 `V011__tools_skills.sql`、`V012__agents.sql`
- [x] T014 [P] 创建 V014 main/capability seed；**V013 无文件、无注册项，不补号**
- [x] T015 [P] 创建 V032 runtime_audit_logs `crates/hiveweb/migrations/V032__runtime_audit_logs.sql`；非 Hook audit tracing + 有界 best-effort DB 双写，Hook 保持 tracing-only
- [x] T016 [P] 创建 V033 `crates/hiveweb/migrations/V033__normalize_workflow_timeout_default.sql`，把 Workflow DB 默认值规范化为 33000 ms
- [x] T017 在 `crates/hiveweb/src/bin/migrate.rs` 注册实际 V001–V033 链，明确跳过 V013，并以 V033 为最后一项

### 历史迁移规划标签（T170–T186，Superseded / 已合并）

> 下列任务号保留用于追溯，但旧“V019–V038 Agent Runtime 扩展”编号不是
> 当前物理迁移，不得按旧文件名新建或重命名 SQL。实际 V019–V031 已被
> global config、game、hook、用户消息和敏感词等后续迁移占用。

- [x] T170 [P] **[Historical/Merged]** `tools.source` 已合并入实际 V011 `V011__tools_skills.sql`
- [x] T171 [P] **[Historical/Merged]** `tools.is_always` 与 CHECK 已合并入实际 V011
- [x] T172 [P] **[Historical/Merged]** `skills.is_always` 已合并入实际 V011
- [x] T173 [P] **[Historical/Remapped]** `recommended_games` 实际由 V015 创建，V029–V031 演进；旧 V022–V025 标签失效
- [x] T174 [P] **[Historical/Merged]** `tools.category_id` 已合并入实际 V011
- [x] T175 [P] **[Historical/Merged]** `skills.category_id` 已合并入实际 V011
- [x] T176 [P] **[Historical/Remapped]** `admin_audit_logs` 实际由 V004 创建；旧 V028 rename 标签失效
- [x] T177 [P] **[Historical/Merged]** Function / Tool `required_capabilities` 已合并入实际 V009 / V011
- [x] T178 [P] **[Historical/Merged]** Workflow `required_capabilities` 已合并入实际 V010
- [x] T179 [P] **[Historical/Merged]** Skill `required_capabilities` 已合并入实际 V011
- [x] T180 [P] **[Historical/Merged]** Workflow `category_id` 已合并入实际 V010；当前 V032 是 runtime audit
- [x] T181 [P] **[Historical/Merged]** Workflow input schema / start description 已合并入实际 V010；当前 V033 只规范 timeout 默认值
- [x] T182 [P] **[Historical/Merged]** Workflow node 类型已合并入实际 V010；不存在物理 V034
- [x] T183 [P] **[Historical/Merged]** Workflow output schema / end description 已合并入实际 V010；不存在物理 V035
- [x] T184 [P] **[Historical/Merged]** answer node / nullable function / node_config 已合并入实际 V010；不存在物理 V036
- [x] T185 [P] **[Historical/Merged]** Capability `category_id` 已合并入实际 V006；不存在物理 V037
- [x] T186 [P] **[Historical/Remapped]** Capability category seed 实际为 V016；不存在物理 V038

### Models（与实际 V005–V012 / V032 行映射）

- [x] T018 [P] Capability/Category/Tag 模型 `crates/hiveweb-admin/src/models/capability.rs`、`models/category.rs`、`models/tag.rs`
- [x] T019 [P] Plugin 模型 `crates/hiveweb-admin/src/models/plugin.rs`（含 `deleted_at`、`sha256`、`s3_key`）
- [x] T020 [P] Function 模型 `crates/hiveweb-admin/src/models/function.rs`（含 kind、plugin_id、schemas JSON）
- [x] T021 [P] Workflow/WorkflowNode/WorkflowEdge 模型 `crates/hiveweb-admin/src/models/workflow.rs`（含 category_id, input_schema, output_schema, required_capabilities）
- [x] T022 [P] Tool 模型 `crates/hiveweb-admin/src/models/tool.rs`（含 source, is_always, category_id, required_capabilities）
- [x] T023 [P] Skill 模型（markdown 模式：content + frontmatter）`crates/hiveweb-admin/src/models/skill.rs`（含 is_always, category_id, required_capabilities）
- [x] T024 [P] Agent 模型 `crates/hiveweb-admin/src/models/agent.rs`（含 model_preset、parent_agent_id、depth）
- [x] T025 [P] **[Legacy/Superseded]** admin ChatSession/ChatMessage 模型 `crates/hiveweb-admin/src/models/chat.rs`（后续已删除）
- [x] T026 [P] RuntimeAuditLog 模型 `crates/hiveweb-admin/src/models/runtime_audit_log.rs`
- [x] T187 [P] RecommendedGame 模型 `crates/hiveweb-admin/src/models/recommended_game.rs`（含 name, reply, reason, game_id, game_name, tag, game_category, game_image, sort_value）
- [x] T188 [P] Admin/AdminAuditLog/LoginRecord 模型 `crates/hiveweb-admin/src/models/admin.rs`、`login_record.rs`（含 role enum）

### Runtime 骨架（不依赖具体 capability handler）

- [x] T027 [P] Capability 静态注册表骨架 `crates/hiveweb-admin/src/runtime/capability.rs`（定义 `Capability` struct、`CapabilityRegistry`、未实现具体 handler；列出 11 个 capability name 常量）
- [x] T028 [P] Instance Pool 骨架 `crates/hiveweb-admin/src/runtime/pool.rs`（历史实现曾包含 `Plugin::reset()` 占位；该设计已由 T250 的 `CompiledPlugin` 缓存 + fresh Store/Instance 契约 supersede）
- [x] T029 [P] Plugin 调用 invoker 骨架 `crates/hiveweb-admin/src/runtime/invoker.rs`（resolve plugin → acquire from pool → invoke → audit；先 stub `invoke()` 返回 unimplemented）
- [x] T030 [P] Workflow 执行器骨架 `crates/hiveweb-admin/src/runtime/workflow.rs`（拓扑 BFS + tokio::join_all 的接口，先 stub）
- [x] T031 [P] Agent 编排骨架 `crates/hiveweb-admin/src/runtime/agent.rs`（接 `crates/agent::AgentRunner` 占位）
- [x] T032 [P] LLM adapter `crates/hiveweb-admin/src/runtime/llm.rs`（启动期读 `llm_presets.toml` → 构造 `HashMap<LlmPresetName, Arc<dyn LLMProvider>>`；提供 `provider_for(&Agent)`）
- [x] T033 在 `crates/hiveweb-admin/src/runtime/mod.rs` 暴露上述子模块，并 `pub use` 关键类型
- [x] T034 在 `crates/hiveweb-admin/src/lib.rs` 增加 `pub mod runtime;`
- [x] T035 在 `crates/hiveweb-admin/src/runtime/startup.rs` 新建 `pub async fn init_runtime_state(config: &Config) -> Result<Arc<RuntimeState>>` 函数，按 plan §Startup Initialization Order 执行；在启动入口初始化 `AppState.runtime_state`。前置失败必须在 listener 前 fail-fast：LLM registry 走静态安全错误 + 非零返回，不 panic；只有明确不可恢复的 builtin/runtime config 路径保留 panic
- [x] T189 [P] Orchestrator `crates/hiveweb-admin/src/runtime/orchestrator.rs`：完整实现 `run_session()` 多 hop loop + tool calling 路由；其中原管理端 SSE 输出面已 superseded，运行时编排仍可被现行调用方复用
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

### 后端契约测试（HTTP REST + host_call ABI；原 admin SSE 项已 superseded）

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

### 🚨 分析 v5 补漏 — FR-032 / SC-009 / FR-027 / FR-029 缺失测试

- [x] T164 [P] [US6] **[Legacy/Superseded]** Integration test `crates/hiveweb/tests/it_chat_session_ownership.rs`：① admin-A 创建 session → admin-B 尝试 POST messages → 403；② admin-A 可正常读写；③ Super 可读其它 admin 的 session 消息；④ 非 Super 读其它 admin 的 session → 403（FR-027 v7 会话所有权与隔离）；该测试随管理端 Chat 删除，不得恢复其 Super 跨会话语义
- [ ] T165 [P] [US1] Integration test `crates/hiveweb/tests/it_plugin_delete_race.rs`：使用唯一 identifier 连续执行 100 轮 HTTP 并发竞态（Thread A 软删除 Plugin、Thread B 新建 Function），每轮断言仅一方成功、失败方为 4093 且不存在“已软删 Plugin 仍被 Function 引用”；另以未提交事务锁定同一 Plugin 行，并通过 `SHOW FULL PROCESSLIST` 确认生产 locking SQL 已进入等待后，受控验证 create-first（`FOR SHARE` → INSERT → COMMIT）和 delete-first（`FOR UPDATE` → `deleted_at` → COMMIT）两种交错。生产两层防御为：① 删除 transaction 持有 `FOR UPDATE` 并统计全部 Function 行；② 创建 transaction 持有 `FOR SHARE` 至 Function/tagging INSERT 提交。实现与集成目标编译完成；本机无 Docker daemon，须等待 disposable MySQL CI 实际 Green 后才可改回 `[x]`（SC-009 race window）
- [ ] T166 [P] [US4] Integration test `crates/hiveweb/tests/it_plugin_memory_limit.rs`：通过 WAT 生成真实 Plugin，经 MySQL → MinIO → InstancePool → Invoker 请求 129 MiB、在 `PLUGIN_CALL_MAX_MEMORY_MB=128` 时验证被强制中止 + 5000 + 安全 tracing + bounded best-effort DB runtime audit + fresh Store/Instance 被丢弃（FR-032）。实现、离线单测和集成目标编译已完成；本机无 Docker daemon，须等待 disposable MySQL/MinIO CI 实际 Green 后才可改回 `[x]`
- [ ] T167 [P] [US4] Integration test `crates/hiveweb/tests/it_wasm_sha256_verify.rs`：① 生产上传正常 Plugin → 记录 DB sha256；② 手动篡改对象存储中的 WASM 文件（翻转 1 byte）；③ 经真实 Invoker → InstancePool 冷加载，宿主 GET 后重算 sha256 ≠ DB 值 → 拒绝编译且不缓存 `CompiledPlugin`、不创建 Store/Instance。真实拒绝路径、清理与 Invoker 早错 exactly-once `outcome=error` 安全审计已实现并完成集成目标编译；本机无 Docker daemon，仍须等待 disposable MySQL/MinIO CI 实际 Green 后才可改回 `[x]`（FR-029 v7 加载前校验）

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
    - `SELECT COUNT(*) FROM functions WHERE plugin_id = ?`（同事务内；Function 仅硬删除，无 `deleted_at`）
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
  - **SC-009 第二层防御**：创建 transaction 执行 `SELECT id, deleted_at FROM plugins WHERE id = ? FOR SHARE`；若已删除 → ROLLBACK + 4093，否则持共享锁完成 Function/tagging INSERT 与 COMMIT（与 T070 的软删除 transaction 排他锁配合）
- [x] T082 [P] [US2] Tool service `crates/hiveweb-admin/src/services/tool.rs`：CRUD + schema 一致性校验（data-model 不变量 #11）— `kind=1` 时 tools.input_schema / output_schema **深度 JSON 等值校验** 与引用 function 的 schema，不一致返 5002 `Schema mismatch`；`kind=2` 时 tools.input_schema 必须能赋值给 workflow 入口 function 的 input_schema（必含所有 required 字段且类型一致）
- [x] T083 [P] [US2] Skill service `crates/hiveweb-admin/src/services/skill.rs`：CRUD（markdown content + 解析 frontmatter）
- [x] T084 [P] [US2] Function API `crates/hiveweb-admin/src/api/function.rs`
- [x] T085 [P] [US2] Tool API `crates/hiveweb-admin/src/api/tool.rs`
- [x] T086 [P] [US2] Skill API `crates/hiveweb-admin/src/api/skill.rs`
- [x] T087 [US2] 把以上 3 个路由 mount 到 router（启动期 ToolRegistry 重建留待 US4 与 invoker 联动）
- [x] T088 [US2] runtime invoker `invoker.rs`：实现 custom Function 调用（按 Function.plugin_id 解析 plugin → pool.acquire → Plugin::call(export, payload)）；超时 / 内存上限按 env 限定；timeout 映射 5004，memory trap 映射 5000，任何调用错误实例均不回池

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
- [x] T103 [US4] 完成 `runtime/pool.rs` / `runtime/invoker.rs`：Extism manifest timeout + `memory.max_pages = memory_mb × 16` + `PluginBuilder::with_fuel_limit`；`PLUGIN_CALL_TIMEOUT_MS` 默认 30000、`PLUGIN_CALL_FUEL` 默认 10000000000，均严格拒绝 0/畸形值；外层 Tokio wall-clock 到期显式调用 `CancelHandle::cancel()`、释放池计数并返回 5004。fuel 耗尽同样返回 5004；trap/OOM/reset 失败及 timeout 实例均不得回池或二次 release。真实无限 CPU loop、外层 cancel、内存边界与池计数离线单测已 Green
- [x] T104 [US4] 完成 `runtime/invoker.rs::invoke()`：把 host_call 注册成 Extism Host Function，闭包内 capture `dispatcher`
- [x] T105 [US4] Audit 写入 `services/runtime_audit.rs::record()`：填 request_id / agent_id / plugin_id / capability / outcome / elapsed_ms / payload_summary（脱敏）

### 前端

- [x] T106 [P] [US4] `web-admin/src/services/capability.ts`：GET capabilities
- [x] T107 [P] [US4] `web-admin/src/components/CapabilityPicker.tsx`：含 dangerous 标记；非 Super 不可勾选 dangerous（hidden = 不渲染）
- [x] T162 [P] [US4] Runtime metrics 端点 `crates/hiveweb-admin/src/api/runtime.rs`：`GET /api/runtime/pool/stats` 返回 global + per_plugin Pool 快照（in_use / idle / created_total / cache_misses / wait_count / `reset_failures` 兼容字段且预期恒为 0），role ≥ System；其中 idle 是不占 permit 的编译缓存，created_total/cache_misses 分别仅统计成功 fresh runtime/成功真实编译；前端 `web-admin/src/pages/DashboardPage.tsx` 加 "Plugin Pool 健康度" 卡片显示 cache_misses 与 wait_count 是否异常

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

**Independent Test**：main 配 `coding-expert` 子 Agent → 通过现行调用方问"Rust 异步"→ runtime audit/tracing 显示 routed → 最终 rust-expert 回复；不依赖已移除的 admin SSE UI。

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

## Phase 8: User Story 6 — 管理端测试聊天（SSE）（历史，已 Superseded）

> 本阶段完整保留任务编号与当时实现事实，仅用于追溯。对应 API/UI/admin 表和测试后来由 `81a84fe` 删除，所有条目均非现行待办，禁止按这些路径恢复。当前用户聊天的同名 runtime/retention 能力属于后续外部 Assistant API，不会使这些 admin 任务重新生效。

**历史 Goal**：管理员在 Web 上输入文字，看到流式增量回复，可见 routed 事件。

**历史 Independent Test（已 superseded）**：输入"你好"→ SSE token 流 → done；输入触发路由的问题 → routed → 子 Agent token → done。

- [x] T126 [P] [US6] **[Legacy/Superseded]** Chat service `crates/hiveweb-admin/src/services/chat.rs`：session CRUD + message persist + history 拉取
- [x] T127 [US6] **[Legacy/Superseded]** Chat API `crates/hiveweb-admin/src/api/chat.rs`：`POST /api/chat/sessions/:id/messages` 返回 `Sse<impl Stream<Item = Event>>`
  - 以下 T127 子项均为已失效的历史契约，不是现行实现要求：
  - **响应 headers**（production-critical 防 reverse proxy 缓冲）：`Cache-Control: no-cache, no-transform` / `Connection: keep-alive` / `X-Accel-Buffering: no`（axum::response::sse::Sse::keep_alive 配合自定义 headers）
  - **Keep-alive**：每 15s 发送 SSE comment `: ping\n\n` 维持连接（绕过中间代理 30s 默认空闲超时）
  - **事件类型**：6 种正向事件（`token` / `tool_call` / `tool_result` / `routed` / `fallback_used` / `done`）+ `error`（与 `done` 互斥，作为异常流终点）
  - **会话所有权校验**（FR-027 v7）：JWT.admin_id == session.admin_id；不匹配 → 403；Super 例外但仍走显式判定
  - **并发限制**（CHK232）：bypass `RateLimit` 中间件；用独立计数器：同一 admin 同时活跃 SSE 流 > `CHAT_SSE_MAX_CONCURRENT_PER_ADMIN`（默认 2）→ 立即返 429 + code 4291
- [x] T128 [US6] **[Legacy/Superseded]** `runtime/orchestrator::run_session()`：history+user msg 多 hop loop + 7 种 SSE 事件（6 种正向 + `error`）；user msg 流前 persist；assistant 由 finalize 写入；中断时 assistant 不入库
- [x] T129 [P] [US6] **[Legacy/Superseded]** `web-admin/src/services/chat.ts`：EventSource wrapper（解析 token / tool_call / routed / done / error）
- [x] T130 [P] [US6] **[Legacy/Superseded]** `web-admin/src/components/ChatStream.tsx`：7 种 SSE 事件渲染（`token` 增量、`tool_call`/`tool_result` 卡片对、`routed` 切换分隔条、`fallback_used` 模型切换提示、`done` 终态、`error` 错误展示）；"正在思考…" 占位（提交后到首事件之间）；30s 无响应超时降级；SSE 中途断开 → "连接中断 + 重新发送" 按钮（user 消息已 persist，assistant 中断内容不写库）
- [x] T131 [US6] **[Legacy/Superseded]** `web-admin/src/pages/ChatPage.tsx`：会话列表 + 当前对话区域
- [x] T132 [P] [US6] **[Legacy/Superseded]** cron 任务 `crates/hiveweb-admin/src/bin/chat_retention.rs`：按 `CHAT_RETENTION_DAYS` 删超期 session（级联清 message）；注册为 `chat-retention` bin target
- [x] T161 [P] [US6] [SC-008] **[Legacy/Superseded]** axe 检测 `web-admin/src/components/__tests__/a11y_chat.test.tsx`：覆盖 ChatPage / ChatStream，0 critical/serious
- [x] T163 [P] [US6] **[Legacy/Superseded]** Integration test `crates/hiveweb/tests/it_sse_concurrency.rs`：3 个测试用例 — 并发限制 (429+4291)、释放后重试成功、per-admin 隔离计数器

**历史 Checkpoint（已 superseded）**：当时完整对话可演示；T043 / T065 / T161 转绿；SC-010 端到端可测。此结果不代表相关组件当前仍应存在。

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
- [x] T168 [P] [SC-010] **[Legacy/Superseded]** 端到端聊天基准 `crates/hiveweb/benches/chat_e2e.rs`：完整 SSE 对话流程（user msg → session create → orchestrator run_session → SSE token/done 事件消费），mock LLM provider 固定 800ms 响应，p95 ≤ 8 秒；结果写入 perf-evidence.md；该 admin Chat 目标不再是 004 现行验收项
- [x] T143 ~~更新 quickstart.md~~ — 已在 analyze 整改阶段直接落地（ModelPreset 下拉、Skill markdown 步骤）
- [x] T144 ~~修订 contracts/api.md Skills~~ — 已在 analyze 整改阶段直接落地（markdown 模式、+ 4094 OptimisticLockConflict、+ 5008 BuiltinSkillProtected）
- [x] T145 [P] 添加 audit log 保留 cron `crates/hiveweb/src/bin/audit_retention.rs`：默认保留 **36500 天（100 年）**（可配 `AUDIT_RETENTION_DAYS` env，与 spec FR-022 对齐），每日扫描清理超期 `runtime_audit_logs` 行；注册为 `audit-retention` bin
- [x] T146 [P] 文档化危险 capability 授予流程 → `specs/004-agent-runtime/SECURITY.md`（capability auth chain / 上传 pipeline 仍现役；原 admin SSE 所有权章节已 superseded）
- [x] T147 [P] 在 `web-admin/src/pages/DashboardPage.tsx` 加 RuntimePoolCard（历史实现包含 reset_failures alert；T250 后该字段仅为兼容保留且预期恒为 0，现行告警关注 cache_misses / wait_count）
- [x] T148 [P] CHANGELOG 更新 + tasks.md 标记 → `specs/004-agent-runtime/CHANGELOG.md`
- [x] T232 [P] 数据种子脚本 `crates/hiveweb-admin/src/bin/seed.rs`：填充测试数据（categories, capabilities, sample plugins/functions/tools/skills/agents）
- [x] T233 [P] 基准种子脚本 `crates/hiveweb-admin/src/bin/seed_bench.rs`：填充 perf bench 数据集
- [x] T234 [P] Super admin 创建脚本 `crates/hiveweb-admin/src/bin/create_super_admin.rs`：命令行创建超级管理员
- [x] T238 [P] Workflow metadata timeout 合同：先补红灯单测，再在
  `crates/hiveweb/src/services/workflow.rs` 的 create/update 强制
  `1000..=330000`，保留 category/capabilities/tags 字段，并同步
  `contracts/api.md`
- [x] T239 [P] Runtime audit 管理端时间筛选：先补 UTC+8 红灯测试，再由
  RangePicker 发送 RFC3339 UTC `toISOString()`，避免八小时偏移
- [x] T240 [P] 以物理 SQL 与 `migrate.rs::MIGRATIONS` 为真值修正文档：
  V001–V033、V013 空号、V032 runtime audit、V033 timeout default；旧
  V019–V038 Agent Runtime 规划标签标记为 Historical/Merged，禁止新建或
  重命名 SQL
- [x] T241 [P] 显式 `RuntimeExecutionContext`：普通 Assistant 链使用
  bounded best-effort DB audit，进入 Hook 后 sticky tracing-only；完整传播
  `request_id` / `session_id`，并以 compile-fail doctest 禁止隐式默认上下文
- [x] T242 [P] Runtime audit 可观测性：tracing 为权威事件流，有界单 worker
  DB 副本暴露 enqueued / persisted / tracing-only / queue drop / writer closed /
  no writer / persist failure 七项指标，pool stats API 与 Dashboard 告警同步展示
- [x] T243 [P] Capability 限流与错误合同：`network.http` 按
  `(plugin_id, session_id)` 最多 8 个并发（无 session 使用共享 sentinel），
  `log.emit` 按 Plugin 使用 100/s token bucket；拒绝统一返回新码 4292，
  保留历史 4291 不复用，并修正 4045 → HTTP 400
- [x] T244 [P] `audit-retention` 最小权限与 fail-fast：只接受专用
  `AUDIT_RETENTION_DATABASE_URL`，连接/清理失败均输出静态安全错误分类并非零
  退出；文档化 DELETE-only 账号并增加 ignored 真实 MySQL 权限测试
- [x] T245 [P] 开发 CLI 安全收敛：`api-test` / `seed` 仅在 `dev-tools`
  feature 下构建；API 测试输出不含 body/签名 URL/secret；Super Admin 与 seed
  密码只允许 stdin 或权限受限文件输入，不含硬编码凭据
- [x] T246 [P] Rust 工具链固定为精确 `1.97.1`：workspace
  `rust-version`、`rust-toolchain.toml` 与 CI 一致；CI 显式校验 rustc/cargo
  release 并使用 `cargo +1.97.1 --locked`
- [x] T247 [P] Workflow 描述字段端到端往返：metadata create/update 持久化
  `end_description`；Graph GET/PUT 的虚拟 start/end 节点往返
  `start_description` / `end_description`，省略时保留旧值；管理端 StartNode /
  EndNode 与详情编辑器可对称查看、编辑并序列化对应描述
- [x] T248 [P] Hook 参数与内部状态边界：Function/Workflow 合并
  `action_params.args`，运行时 `_agent_context` 快照覆盖用户同名字段；
  `_agent_context_updates` 只用于受控内存更新，所有公开 Workflow end-output
  路径剥离该保留键，output schema 不得声明该键
- [x] T249 [P] Hook 出站与关联边界：Function/Workflow 输入按
  `args < HookContext < _agent_context` 合并可信运行时字段；Webhook 创建、
  更新、首次调用和重试统一执行 HTTPS、DNS 全答案公网校验、DNS pinning、
  禁代理/重定向与 header 校验；HTTP middleware 仅接受非 nil 标准 UUID 并
  规范化 `X-Request-Id`；DNS 错误类型化为 `Policy` 与
  `ResolveUnavailable`，Hook 仅重试 `ResolveUnavailable`、reqwest
  `is_connect()` 与 attempt timeout；Policy/header/client build/其他
  Request/HTTP non-2xx 均不重试
- [x] T250 [P] Runtime 拒绝/早错安全：permission DB failure 与 Plugin
  lookup/pool/S3/SHA/compile 等早期失败通过 RAII guard 恰好写一条安全审计；
  `log.emit` 令牌桶使用单调时钟并覆盖乱序时间；Extism 显式禁用 WASI，并以
  真实 WASM 覆盖 host_call 可用与 WASI import 拒绝；Pool 强制
  max_per_plugin/max_total、FIFO acquire、`PoolBusy`/5009 timeout、wait_count、
  cancellation-safe permit 与 idle-reaper lifecycle；只缓存 CompiledPlugin，
  每次调用 fresh Store/Instance，memory/global/table 不跨调用
- [x] T251 [P] 运维 CLI 深化加固：`seed-bench` 独立 `bench-tools` gate、
  disposable DB 解析/精确 destructive 确认与随机密码；密码/请求体文件用
  `O_NOFOLLOW` 单次打开后的 fd metadata 校验；`api-test` 支持 stdin/安全文件；
  `chat-retention` 只接受正数配置且连接/清理失败静态报错并非零退出；
  正式密码统一按 6..=20 Unicode code point + ASCII letter/digit 校验
- [x] T252 [P] Workflow 公开结果与 Graph 合同：无 `output_schema` 时单终端
  直接返回、多终端按稳定 `node_key -> output` 返回并剥离内部更新；管理端补齐
  `start_description` 展示/编辑/保存；Graph GET 示例包含虚拟 start/end 节点、
  `node_type` 与 `src_node_key` / `dst_node_key`；scalar schema 保持 scalar，
  UI End 只展示 outputs，HTTP node_results 顶层剥离内部更新；v2 仅在 Graph
  刷新与 v3 写入均成功后迁移，失败保留 v2 且不写 v3
- [x] T253 [P] LLM registry 启动 fail-fast：缺失/不可读配置、无效 TOML、
  default 数量不为 1 或 default provider 无效时，在 Router/listener 前以静态
  安全错误非零退出，不再回退空 registry；provider 默认必须 api_key_env，
  仅 `auth="none"` + 显式合法 http(s) base_url 免认证且与 key 互斥；
  无效非 default preset 静态告警并跳过
- [x] T254 [P] 003 性能复现文档安全化：
  `specs/003-admin-center/perf-evidence.md` 的所有 `seed-bench` 命令使用
  `bench-tools`、从 `DATABASE_URL` 解析的 `_test`/`_bench` database 名和精确
  `--confirm-destructive`；移除固定管理员密码，bootstrap 密码只走 stdin，
  curl/hey 只接收 mode-0600 临时 body 文件路径
- [x] T255 [P] Runtime LLM 全调用面改用 `LlmRegistry::build_chain`：
  orchestrator、`llm.invoke`、generate-answer、Tool/Skill test 与 builtin LLM
  路径从启动期 registry 取得多 provider preset 的完整 primary +
  `providers[1..]` fallback chain。六处生产接线统一使用 bounded
  `chat_with_options` / `chat_stream_with_options`，且不会退化为
  `build_primary`、primary-only 或整链外层 retry；调用面 wiring contract 与
  focused tests 已 Green。启动期构造/缓存生命周期和同名 model 的稳定 identity
  已由 T259 的 Red → Green 证明
- [x] T256 [P] LLM startup 文档与现有入口收敛：registry 缺失/不可读、TOML
  无效、default 数量不为 1 或 default provider 无效时，在 listener 前仅输出
  静态 `llm_registry_load_failed` 并非零返回，不 panic；成功日志写
  `default=cheap-fast` 而非 `Some("cheap-fast")`。确实不可恢复的 builtin /
  Plugin runtime config panic 路径保持独立
- [x] T257 [P] Pool admission/metrics 最终语义：per-Plugin/global capacity
  只计算 `in_use + reserved`，idle `CompiledPlugin` cache 不占 permit；
  `cache_misses` 仅真实成功编译后增加，`created_total` 仅 fresh Store/Instance
  成功创建后增加；失败回滚与跨 Plugin idle-cache 场景测试 13/13 Green
- [x] T258 [P] Graceful shutdown：SIGINT/SIGTERM 停止 accept，最多 30 秒
  drain 活跃请求，随后停止并等待 idle reaper、drop Instance Pool/编译缓存、
  关闭 DB pool；drain timeout 仍继续清理。真实本地 server 的 stop-accept /
  drain / force-drop / streaming-body / resource-drop 测试 6/6 Green，reaper
  lifecycle 3/3、`chat_assistant` 非 ignored 测试 17/17 Green（8 项 infra 测试
  保持 ignored）
- [x] T259 LLM chain 生命周期与稳定 identity（批准 30A；T255 与
  T260–T263 前置）完成 Red → Green：registry load 在启动期构造并缓存完整
  chain，重复 `build_chain` 只克隆同一 `Arc`，生产代码不存在
  `build_primary`；两个不同 backend / fallback 条目使用相同 model 时仍按
  `(preset name, providers[] ordinal)` 命中正确 provider。实现与 focused
  registry/provider/wiring tests 已 Green
- [x] T260 LLM preset 默认值与显式请求覆盖（批准 31A；依赖 T259）完成
  Red → Green：省略 `max_tokens`/`temperature` 使用命名 preset 默认，合法显式
  值经过 primary → fallback 保持不变；唯一输入例外是 `llm.invoke`
  显式 `max_tokens=0`，其在 DB/registry/provider 前以 4001 无效参数失败，
  不得被当作省略值
- [x] T261 LLM 结果与 fallback 可观测性（批准 32A；依赖 T259）完成
  Red → Green：成功结果返回 `actual_model`/`fallback_used`/`reason`，provider
  切换写脱敏 tracing + best-effort `llm_fallback` audit；
  `generate_answer_node` 本地文本兜底写 `llm_local_fallback`、
  `fallback_used=false`、`actual_model=NULL`。实现覆盖
  `crates/providers/src/{base.rs,fallback_provider.rs}` 与
  `crates/hiveweb/src/services/runtime_audit.rs`、
  `crates/hiveweb/src/runtime/workflow/generate_answer_node.rs`、
  `crates/hiveweb/src/runtime/{orchestrator.rs,tool_test.rs,skill_test.rs}`、
  `crates/hiveweb/src/runtime/capabilities/llm.rs` 和
  `crates/hiveweb/src/runtime/builtins/game_info.rs`；静态 reason 不携带原始错误，
  provider/audit/focused runtime tests 已 Green
- [x] T262 LLM deadline 分层（批准 33A；依赖 T259）完成 Red → Green：
  paused-time / deterministic fake provider 测试覆盖每节点 25 秒、普通整链
  45 秒、`llm.invoke` 整链 25 秒以及 deadline 后不启动下一 provider；
  实现位于
  `crates/providers/src/fallback_provider.rs`、
  `crates/hiveweb/src/runtime/llm.rs` 与
  `crates/hiveweb/src/runtime/capabilities/llm.rs`；timeout 使用 typed 分类并为
  Plugin 外层保留 5 秒清理预算，provider/capability tests 已 Green
- [x] T263 显式未知 preset fail-closed（批准 34A；依赖 T259）完成
  Red → Green：`build_chain(None)` 使用 default，而
  `build_chain(Some("unknown"))`、
  已删除/跳过 preset 的 Agent、Workflow generate-answer、Tool/Skill test 与
  `llm.invoke` 均返回 5007 / typed `ModelPresetUnknown` 且不请求 default
  provider。`llm.invoke` 仅在**存在的 Agent DB 行**中 `model_preset IS NULL`
  时使用 default；缺失 Agent 行在 build/provider 前安全失败。实现覆盖
  `crates/hiveweb/src/runtime/llm.rs`、
  `crates/hiveweb/src/runtime/{orchestrator.rs,tool_test.rs,skill_test.rs}`、
  `crates/hiveweb/src/runtime/workflow/generate_answer_node.rs`、
  `crates/hiveweb/src/runtime/capabilities/llm.rs`、
  `crates/hiveweb/src/runtime/builtins/game_info.rs` 与
  `crates/hiveweb/src/services/agent.rs`，并保持 Agent POST/PUT 的现有 5007
  校验；registry、Workflow、Capability、builtin 和三个 runtime 调用面直接
  行为测试已 Green

---

## Dependencies & Execution Order

### Phase 依赖

- Phase 1 → Phase 2 → Phase 2.5 必须先红灯 → Phase 3+
- Approved LLM remediation order was T259 → (T260, T261, T262, T263);
  T259 与 T255–T263 均已按共享文件串行合并并完成各自 Red → Green
- US1 / US2 / US4 互相基本独立（US2 的 ToolRegistry 注册依赖 US4 的 invoker 完成；US2 的 custom function 调用依赖 US4 的 dispatcher） → 推荐顺序 US1 → US4 → US2，或 US1 / US4 并行后再 US2
- US3 依赖 US2（节点是 Function）
- US5 依赖 US2 + US4（Agent 用 Tool 与 Capability）
- US6 原 admin Chat 曾依赖 US5；该阶段现已 superseded
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
- 原“加 US6 SSE chat → demo 完整对话”增量已 superseded，不恢复
- 加 US7 Category/Tag → 运营组织能力
- Polish → 性能 / a11y / 文档

---

## Task Summary

- **Total Historical Tasks**: 237（计数保留；其中 US6、T163/T164/T168 等 admin Chat 项已 superseded）
- **2026-07-24 remediation**: 21（T238–T258，Workflow timeout/描述往返、
  审计时间与迁移真值、显式执行上下文、审计指标、Capability 限流、retention
  与运维 CLI 安全、精确 Rust 1.97.1、Hook 参数/SSRF/关联边界、early audit /
  单调时钟 / WASI、Workflow 公共输出、LLM registry/fallback、Pool metrics
  与 graceful shutdown；T238–T258 全部完成）
- **2026-07-24 approved LLM follow-up**: 5（T259–T263 全部完成；T259
  先完成启动期 chain 缓存与稳定 identity，再收敛 T255/T260–T263）
- **Current checkbox status**: 259 / 262 完成，3 项 Pending；仅
  T165/T166/T167 等待 disposable MySQL/MinIO CI（本机 Docker daemon
  unavailable）
- **Setup (Phase 1)**: 5 + 1（T149 集中 15 个 env vars）
- **Foundational (Phase 2)**: 31 + 3 跨切乐观锁（T150/T151/T152）= 34
- **Historical migration planning labels**: T170–T186 已标记为
  Superseded/Merged；它们不是物理 V019–V038。当前注册链为 V001–V033，
  V013 保留空号，V032/V033 分别为 runtime audit / Workflow timeout 默认值
- **Tests (Phase 2.5)**: 33 + 1 SSE 并发（T163）= 34；已由 interleaved 实现覆盖（`it_dispatcher.rs` / `contract_plugin.rs` 等）；**分析 v5 补漏 4 项**：T164 会话所有权 / T165 race window / T166 memory limit / T167 sha256 校验
- **Admin Chat 历史说明**: 上述历史计数中的 T163/T164 已 superseded；其余覆盖记录保持不变
- **US1 Plugin**: 9 + 1 a11y（T157）
- **US2 Function/Tool/Skill**: 16 + 1 a11y（T158）
- **US4 Capability**: 13 + 1 pool/stats 端点（T162）
- **US3 Workflow**: 8 + 1 a11y（T159）
- **US5 Agent**: 10 + 1 a11y（T160）
- **US6 admin Chat**: 历史 7 + 1 a11y（T161），全部 superseded
- **US7 Category/Tag**: 4
- **Admin Center / Dashboard / RecommendedGame (Phase 9.5)**: 32（T193–T224）
- **Polish (Phase 10)**: 历史计数 12 + 5 perf bench（T153–T156 + T168）= 17；其中 T168 已 superseded

**Parallel Opportunities**: 180+ 任务标 [P]
**Independent MVP**: US1 + US2 + US4（共 38 + 1 metrics 实现任务 + 33 红灯 + 3 乐观锁 + 1 env 配置）

### Analyze v3 整改任务覆盖矩阵（CHK gap 到 task 映射）

| Spec / Doc 更新 | 影响 task | 整改方式 |
| --- | --- | --- |
| 8 个新 env var | T149 | 描述扩写到 15 项 |
| GET /api/runtime/pool/stats 端点 | T162 新增 | US4 加 1 任务 |
| 原 admin SSE 6 事件（+tool_result/+fallback_used）+ headers + 15s ping | T065, T127, T130 | 历史整改，已 superseded |
| SC-009 race window FOR UPDATE | T070 | 描述扩写到 5 个子步骤 |
| WASM imports 静态检查 | T037, T070 | 测试 + 实现描述都扩写 |
| Startup Init Order 12 步 | T035 | 描述扩写 |
| 原 admin `chat_sessions` snapshot 列 | T014 | 历史整改，已 superseded |
| Tool schema 深度等值 | T040, T082 | 测试 + 实现描述都扩写 |
| 5009 PoolBusy 错误码 | T050 | 测试描述扩写覆盖 |
| 4291 admin SSE concurrency 错误码 | T163 新增 | 历史整改，已 superseded |
| workflow edge mapping schema | T108 | 描述扩写 |

### Analyze v5 整改任务覆盖矩阵（cross-artifact gap 到 task 映射）

| Spec Gap | 影响 task | 整改方式 |
| --- | --- | --- |
| FR-027 原 admin 会话所有权（admin_id 校验） | T164 新增 | 历史整改，已 superseded |
| FR-032 Plugin memory limit 128 MiB | T166 新增 | US4 加真实 WAT memory-limit 集成测试；基础设施 CI Green 前保持 Pending |
| SC-009 race window 并发防护 | T165 新增 | US1 加 delete race 集成测试；基础设施 CI Green 前保持 Pending |
| FR-029 WASM 加载前 sha256 校验 | T167 新增 | US4 加真实 Invoker/Pool sha256 mismatch 集成测试；audit 与基础设施 CI Green 前保持 Pending |
| T037-T069 interleaved 覆盖 | 不适用 | 已清理 strikethrough 语法，统一为 `[x]` + 注释说明 |

### Analyze v6 整改任务覆盖矩阵

| Spec Gap | 影响 task | 整改方式 |
| --- | --- | --- |
| SC-010 原 admin 端到端聊天无 perf bench | T168 新增 | 历史整改，已 superseded |
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
- **当前状态**：保留 237 个历史任务的编号与计数，并单列 T238–T258
  remediation；Legacy/Superseded 的 admin Chat 与 RecommendedGame 条目不属于
  当前交付面。当前 262 个任务条目中 259 个为 `[x]`；仅
  T165/T166/T167 因本机 Docker daemon unavailable、尚未取得 disposable
  MySQL/MinIO CI Green 而保持 `[ ]` Pending，不得以历史计数推断这三项完成。
  T103 的 fuel + 外层 wall-clock cancel 已由离线真实 WASM 测试覆盖并完成。
- **新增实体**（未在原始 spec 中但已实现）：`RecommendedGame`（推荐游戏管理）、`Admin`/`AdminAuditLog`/`LoginRecord`（003-admin-center）、`Dashboard`（统计面板）、`Capability`（CRUD 管理）、`RuntimeAuditLog`（运行时审计日志）
- **新增页面**：Dashboard、Capability、LoginPage、AdminPage、AdminAuditLogPage、RuntimeAuditLogPage、LoginRecordPage、RecommendedGamePage
- **新增运行时组件**：orchestrator、builtins、builtin_tools、wasm_exports
- **新增 bin 工具**：seed、seed_bench、create_super_admin（另有 migrate/audit_retention；当前 `chat_retention` 面向普通用户表，不恢复 admin Chat）
- **Admin/RBAC/审计**：003-admin-center 的 Admin CRUD、LoginRecord、AdminAuditLog、RBAC 权限、Super Admin 保护已在 Phase 9.5 中记录；相关测试 contract_admin.rs / contract_auth.rs / contract_dashboard.rs / it_rbac.rs / it_login_record.rs / it_lockout.rs / it_super_admin_guard.rs 已记录在 Phase 2.5
