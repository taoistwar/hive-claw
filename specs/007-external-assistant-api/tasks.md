---
description: "Task list for External Assistant API"
---

# Tasks: 对外 Assistant API

**Input**: Design documents from `/specs/007-external-assistant-api/`
**Prerequisites**: plan.md (required), spec.md (required), contracts/api.md

**Tests**: 🔴 测试为强制要求（宪法 Principle II — NON-NEGOTIABLE）。集成测试（T019）必须在任务全部完成后通过。

**Organization**: 按 user story 分组，每个 story 可独立交付与验证。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: 可并行
- **[Story]**: 所属用户故事（US1..US5）
- 描述中含**确切**文件路径

---

## Phase 1: Setup（外部 DB 连接 + 路由）

**Purpose**: 外部数据库连接池、AppState 扩展、公开路由注册。

- [x] T001 [P] 修改 `crates/hiveweb/src/api/mod.rs` — `AppState` 新增 `ext_pool: Option<MySqlPool>` 字段
- [x] T002 [P] 修改 `crates/hiveweb/src/api/mod.rs` — `create_router()` 接受 `ext_pool: Option<MySqlPool>` 参数并传入 `AppState`
- [x] T003 修改 `crates/hiveweb/src/main.rs` — 从环境变量 `EXTERNAL_DB_URL` 创建外部只读 MySQL 连接池，传入 `create_router()`
- [x] T004 修改 `crates/hiveweb/src/api/mod.rs` — `mod assistant;` + 注册 `assistant::router()` 到 `public_routes`

---

## Phase 2: US1 + US2 — 核心逻辑（已完成）

**Purpose**: 用户校验 + LLM 调用核心流程。US1 和 US2 共享同一请求路径。

- [x] T005 [US1] 创建 `crates/hiveweb/src/api/assistant.rs` — 路由（`POST /assistant`）、`AssistantRequest`/`AssistantResponse` 结构体、外部 DB 模型（`CloudUser`、`CcUserMembership`）
- [x] T006 [US2] 实现 `user_exists_in_cloud()` — 查询外部 `cloud_user` 表 `WHERE ID = ?`，不存在返回 false（FR-003/FR-004）
- [x] T007 [US1] 实现 `check_vip_membership()` — 查询外部 `cc_user_membership` 表，判定 VIP 有效性（FR-005）
- [x] T008 [US4] 实现 `ensure_internal_user()` — 内部 `users` 表同步：不存在则 `INSERT`（phone=`assistant_{id}`，bcrypt hash "test"）（FR-010）
- [x] T009 [US1] 实现 `call_main_agent()` — 加载 `agents` 表中的 Main Agent（id=1），使用 `crates/agent` 的 `AgentRunner` 执行编排（system_prompt + LLM 多轮），收集最终文本返回（FR-011）
- [x] T010 [US1] 实现 `assistant_chat()` handler — 串联校验→限流→同步→LLM 主流程，返回 `AssistantResponse`（FR-001/FR-002/FR-013）

---

## Phase 3: US3 — 每日限流（已完成）

**Purpose**: Redis 日访问次数限制，区分 VIP/普通用户。

- [x] T011 [US3] 实现 `get_config_number()` — 从 `global_configs` 表读取 `vip_ask_times` / `normal_ask_times`（FR-006/FR-007）
- [x] T012 [US3] 实现 `check_and_incr_daily_limit()` — Redis `INCR` + 首次 `EXPIRE`（到次日 0 点）+ 超额检查（FR-008/FR-009）
- [x] T013 [US3] 实现 `seconds_until_midnight()` — 计算到当天 23:59:59 的剩余秒数

---

## Phase 4: 待完成（Clarify 新增/修改）

**Purpose**: Clarify 阶段决议的签名鉴权、参数校验、边界修复。

### 签名鉴权 (US1 — FR-014/FR-015)

- [x] T014 [US1] 新增 `verify_sign()` 到 `crates/hiveweb/src/api/assistant.rs` — 从环境变量 `ASSISTANT_SECRET` 读取预共享密钥，计算 `MD5(secret + "/api/assistant" + "?body=" + body)`，与请求 query param `sign` 比对
- [x] T015 [US1] 在 `assistant_chat()` handler 开头新增 Content-Type 校验 — 必须为 `application/json; charset=UTF-8`（FR-015）
- [x] T016 [US1] 在 `assistant_chat()` handler 开头新增签名校验调用 — 签名不匹配返回 `"Invalid signature"` 错误（FR-014）

### 参数校验 (US1 — FR-016/FR-018)

- [x] T017 [US1] 在 `assistant_chat()` handler 开头新增 `user_id <= 0` 校验 — 直接返回 `"user_id must be positive"` 错误，不查询外部 DB（FR-018）
- [x] T018 [US1] 在 `assistant_chat()` handler 开头新增 `message.is_empty()` 校验 — 直接返回 `"message must not be empty"` 错误，不消耗配额（FR-016）

### 错误处理修复 (US1 — FR-017)

- [x] T019 [US1] 修改 `assistant_chat()` 中 LLM 失败处理 — 调用失败时执行 Redis `DECR` 回滚计数器，返回 `"Service busy, please retry later"`（FR-017）

### 边界条件修复 (Clarify Q3)

- [x] T020 [US1] 修改 `check_vip_membership()` 中 `effective_end_time` 判定 — 从 `end > now` 改为 `end >= now`（Clarify Session Q3 决议）

### 配置文档

- [x] T021 [P] 更新 `.env.example` — 添加 `EXTERNAL_DB_URL` 和 `ASSISTANT_SECRET` 的环境变量声明及注释

---

## Phase 5: US5 — 降级处理（已完成）

**Purpose**: 外部服务不可用时的优雅降级。

- [x] T022 [US5] 实现外部 DB 连接检查 — `ext_pool.is_none()` 时返回 `"Assistant service unavailable"`（FR-012）
- [x] T023 [US5] 实现 Redis 不可用降级 — `get_multiplexed_async_connection()` 失败时返回 `"Redis unavailable"`，不继续处理

---

## Phase 6: 集成测试

**Purpose**: 端到端验证所有 FR 和边缘用例。

- [x] T024 🔴 [US1] 单元测试 — 合法用户成功请求（签名+JSON序列化+响应格式）
- [x] T025 🔴 [US1] 单元测试 — 签名校验（正确匹配/错误签名/篡改body/空签名/大小写）
- [ ] T026 🔴 [US2] 集成测试 — 用户不存在（不存在于 cloud_user → `"User not found"`）⚠️ 需外部DB
- [x] T027 🔴 [US1] 单元测试 — 参数校验（user_id=0/-1 reject, user_id=1 pass）
- [x] T028 🔴 [US1] 单元测试 — 参数校验（message="" reject, message="hello" pass）
- [ ] T029 🔴 [US3] 集成测试 — 限流超额（INCR 达到上限 → `"Daily limit reached"`）⚠️ 需Redis
- [ ] T030 🔴 [US5] 集成测试 — 外部 DB 不可用（未配置 EXTERNAL_DB_URL → `"Assistant service unavailable"`）⚠️ 需外部DB
- [ ] T031 🔴 [US4] 集成测试 — 内部用户同步（首次 access → users 表新增记录）⚠️ 需MySQL
- [ ] T032 🔴 [US1] 集成测试 — LLM 超时（配额是否回滚）⚠️ 需完整基础设施
- [x] T033 🔴 [US1] 单元测试 — 时间/TTL 计算（seconds_until_midnight >0 且 ≤86400）
- [ ] T034 🔴 [US3] 集成测试 — 跨天重置 ⚠️ 需完整基础设施
- [ ] T035 🔴 [US3] 集成测试 — 并发限流准确性 ⚠️ 需完整基础设施

---

## Dependency Graph

```
Phase 1 (Setup) ──→ Phase 2 (Core) ──→ Phase 3 (Limit) ──→ Phase 4 (Clarify fixes)
                     │                    │
                     └─→ Phase 5 (Degrade)                    Phase 6 (Tests)
```

- Phase 2/3/5 均在 setup 完成后并行开发（同一模块 `assistant.rs`，但函数独立）
- Phase 4 修改已有函数，依赖 Phase 2/3 完成
- Phase 6 在所有实现完成后运行

## Task Summary

| Category | Total | Done | Remaining |
|----------|-------|------|-----------|
| Setup (Phase 1) | 4 | 4 | 0 |
| Core Logic (Phase 2) | 6 | 6 | 0 |
| Rate Limit (Phase 3) | 3 | 3 | 0 |
| Clarify Fixes (Phase 4) | 8 | 8 | 0 |
| Degrade (Phase 5) | 2 | 2 | 0 |
| Tests (Phase 6) | 12 | 6 | 6 |
| **Total** | **35** | **29** | **6** |
