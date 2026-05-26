---

description: "Task list for admin center implementation"
---

# Tasks: 管理中心

**Input**: Design documents from `/specs/003-admin-center/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Tests**: 测试为强制要求（宪法 Principle II – NON-NEGOTIABLE）。每个用户故事的实现任务之前必须先编写并红灯运行对应的契约测试与集成测试，再进入实现阶段。

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

- **Backend (Rust)**: `crates/hiveweb/src/`
- **Frontend (TypeScript/React)**: `web/src/`
- **Shared**: Specs in `specs/003-admin-center/`

<!--
  ============================================================================
  Tasks are organized by user story from spec.md:
  - User Story 1 (P1): 管理员登录
  - User Story 2 (P1): 管理员账号管理
  - User Story 3 (P2): 管理员角色权限管理
  - User Story 4 (P2): 仪表盘概览
  ============================================================================
-->

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization and basic structure

- [x] T001 [P] Create `crates/hiveweb/` directory structure per plan.md
- [x] T002 [P] Create `web/` directory structure per plan.md
- [x] T003 [P] Initialize Rust workspace in `crates/hiveweb/Cargo.toml`
- [x] T004 [P] Initialize npm package in `web/package.json`
- [x] T005 [P] Configure Rust linting: `cargo fmt` and `cargo clippy`
- [x] T006 [P] Configure TypeScript linting: ESLint + Prettier
- [x] T007 [P] Setup MySQL database: `CREATE DATABASE hiveweb` (provisioned via `scripts/docker-compose.yml`)
- [x] T008 [P] Setup Redis: install and configure connection (provisioned via `scripts/docker-compose.yml`)
- [x] T009 [P] Setup Rustfs/S3: configure local MinIO or S3 endpoint (MinIO + bucket init in `scripts/docker-compose.yml`)

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core infrastructure that MUST be complete before ANY user story can be implemented

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T010 [P] Create database migration framework using SQLx
- [x] T011 Create `admins` table migration (V001__create_admins_table.sql)
- [x] T012 Create `login_records` table migration (V002__create_login_records_table.sql)
- [x] T013 [P] Implement MySQL connection pool in `crates/hiveweb/src/db/connection.rs`
- [x] T014 [P] Implement Redis connection pool in `crates/hiveweb/src/cache/redis.rs`
- [x] T015 [P] Implement Rustfs/S3 client in `crates/hiveweb/src/storage/s3.rs`
- [x] T016 [P] Implement JWT authentication middleware in `crates/hiveweb/src/middleware/auth.rs`
- [x] T017 [P] Create Admin model in `crates/hiveweb/src/models/admin.rs`
- [x] T018 [P] Create LoginRecord model in `crates/hiveweb/src/models/login_record.rs`
- [x] T019 [P] Create Role enum and permissions in `crates/hiveweb/src/models/role.rs`
- [x] T020 [P] Implement password hashing with bcrypt in `crates/hiveweb/src/utils/password.rs`
- [x] T021 [P] Create base API response types in `crates/hiveweb/src/api/response.rs`
- [x] T022 [P] Setup axum router in `crates/hiveweb/src/api/mod.rs`
- [x] T023 [P] Create frontend API service in `web/src/services/api.ts`
- [x] T024 [P] Create frontend auth service in `web/src/services/auth.ts`
- [x] T025 [P] Create React Router setup in `web/src/App.tsx`
- [x] T026 [P] Create Layout component in `web/src/components/Layout.tsx`

**Checkpoint**: Foundation ready - user story implementation can now begin in parallel

---

## Phase 2.5: Tests (Red Phase) — 必须先于实现编写

**Purpose**: 在任何 user story 实现之前编写契约/集成测试，按宪法 Principle II 执行 TDD（Red → Green → Refactor）。

**⚠️ CRITICAL**: 这些测试必须先观察到失败，再开始对应的实现任务。

### Contract tests (后端 axum handler 契约)

- [ ] T026a [P] Contract test: `POST /api/auth/login` 成功/失败/锁定 三类响应在 `crates/hiveweb/tests/contract_auth.rs`
- [ ] T026b [P] Contract test: `GET /api/auth/me` 已认证/未认证/Token 过期 在 `crates/hiveweb/tests/contract_auth.rs`
- [ ] T026c [P] Contract test: `GET /api/admins` 分页参数 + 各角色权限 在 `crates/hiveweb/tests/contract_admin.rs`
- [ ] T026d [P] Contract test: `POST /api/admins` 唯一性冲突/手机号格式校验 在 `crates/hiveweb/tests/contract_admin.rs`
- [ ] T026e [P] Contract test: `PUT /api/admins/:id`、`DELETE /api/admins/:id`、`PATCH /api/admins/:id/status` 在 `crates/hiveweb/tests/contract_admin.rs`
- [ ] T026f [P] Contract test: `GET /api/dashboard/stats` 与 `GET /api/dashboard/recent-logins` 在 `crates/hiveweb/tests/contract_dashboard.rs`

### Integration tests (跨层场景)

- [ ] T026g [P] Integration test: 登录失败 5 次后账号锁定 15 分钟（FR-017）在 `crates/hiveweb/tests/it_lockout.rs`
- [ ] T026h [P] Integration test: 角色权限隔离 100% 准确（SC-007）— Normal / System / Super 各发起越权请求均返回 403 在 `crates/hiveweb/tests/it_rbac.rs`
- [ ] T026i [P] Integration test: 不能删除/禁用最后一个 Super 管理员（FR-012、FR-009）在 `crates/hiveweb/tests/it_super_admin_guard.rs`
- [ ] T026j [P] Integration test: 登录成功后 `last_login_at` 与 `login_records` 同步写入（FR-003）在 `crates/hiveweb/tests/it_login_record.rs`

### Frontend component tests (Vitest + Testing Library)

- [x] T026k [P] Component test: LoginForm 表单校验 + 错误展示 在 `web/src/components/__tests__/LoginForm.test.tsx`（4/4 通过）
- [x] T026l [P] Component test: PermissionGuard 隐藏/重定向逻辑 在 `web/src/components/__tests__/PermissionGuard.test.tsx`（4/4 通过）
- [x] T026m [P] Component test: AdminTable 分页与角色按钮可见性 在 `web/src/components/__tests__/AdminTable.test.tsx`（5/5 通过）

**Checkpoint**: 所有 T026a–T026m 必须先以红灯状态提交（CI 标记失败），随后实现任务方可开始。

---

## Phase 3: User Story 1 - 管理员登录 (Priority: P1) 🎯 MVP

**Goal**: 管理员可以使用手机号和密码登录系统，登录成功后更新最后登录时间

**Independent Test**: 可以独立测试登录流程，包括验证、会话创建、最后登录时间更新

### Implementation for User Story 1

- [x] T027 [P] [US1] Create LoginRequest/ LoginResponse DTOs in `crates/hiveweb/src/api/auth.rs`
- [x] T028 [US1] Implement login handler in `crates/hiveweb/src/api/auth.rs`
- [x] T029 [US1] Implement password validation in `crates/hiveweb/src/utils/password.rs`
- [x] T030 [US1] Implement JWT token generation in `crates/hiveweb/src/middleware/auth.rs`
- [x] T031 [US1] Implement login failure tracking with Redis in `crates/hiveweb/src/services/auth.rs`
- [x] T032 [US1] Update last_login_at on successful login in `crates/hiveweb/src/models/admin.rs`
- [x] T033 [US1] Implement account lock/unlock logic in `crates/hiveweb/src/services/auth.rs`
- [x] T034 [US1] Create LoginForm component in `web/src/components/LoginForm.tsx`
- [x] T035 [US1] Create LoginPage in `web/src/pages/LoginPage.tsx`
- [x] T036 [US1] Implement useAuth hook in `web/src/hooks/useAuth.ts`
- [x] T037 [US1] Add login API call in `web/src/services/auth.ts`
- [x] T038 [US1] Store JWT token in localStorage/cookie in `web/src/utils/auth.ts`
- [x] T039 [US1] Implement auto-redirect after login in `web/src/pages/LoginPage.tsx`

**Checkpoint**: At this point, User Story 1 should be fully functional and testable independently

---

## Phase 4: User Story 2 - 管理员账号管理 (Priority: P1)

**Goal**: 系统管理员和超级管理员可以添加、删除、修改、禁用/启用管理员账号

**Independent Test**: 可以独立测试管理员的增删改查操作

### Implementation for User Story 2

- [x] T040 [P] [US2] Create AdminRequest/AdminResponse DTOs in `crates/hiveweb/src/api/admin.rs`
- [x] T041 [P] [US2] Create admin list API endpoint in `crates/hiveweb/src/api/admin.rs`
- [x] T042 [P] [US2] Create get admin by ID endpoint in `crates/hiveweb/src/api/admin.rs`
- [x] T043 [P] [US2] Create create admin endpoint in `crates/hiveweb/src/api/admin.rs`
- [x] T044 [P] [US2] Create update admin endpoint in `crates/hiveweb/src/api/admin.rs`
- [x] T045 [P] [US2] Create delete admin endpoint in `crates/hiveweb/src/api/admin.rs`
- [x] T046 [P] [US2] Create toggle admin status endpoint in `crates/hiveweb/src/api/admin.rs`
- [x] T047 [US2] Implement admin service layer in `crates/hiveweb/src/services/admin.rs`
- [x] T048 [US2] Add phone uniqueness validation in `crates/hiveweb/src/services/admin.rs`
- [x] T049 [US2] Implement "cannot delete super admin" check in `crates/hiveweb/src/services/admin.rs`
- [x] T050 [US2] Implement "cannot disable last super admin" check in `crates/hiveweb/src/services/admin.rs`
- [x] T051 [P] [US2] Create AdminTable component in `web/src/components/AdminTable.tsx`
- [x] T052 [P] [US2] Create AdminForm component in `web/src/components/AdminForm.tsx`
- [x] T053 [US2] Create AdminPage in `web/src/pages/AdminPage.tsx`
- [x] T054 [US2] Implement useAdmin hook in `web/src/hooks/useAdmin.ts`
- [x] T055 [US2] Add admin CRUD API calls in `web/src/services/admin.ts`
- [x] T056 [US2] Implement pagination in AdminTable component
- [x] T057 [US2] Add confirm dialogs for delete/disable actions

**Checkpoint**: At this point, User Stories 1 AND 2 should both work independently

---

## Phase 5: User Story 3 - 管理员角色权限管理 (Priority: P2)

**Goal**: 不同角色的管理员登录后看到不同的功能菜单和操作权限

**Independent Test**: 可以独立测试不同角色的权限边界

### Implementation for User Story 3

- [x] T058 [P] [US3] Define role permissions map in `crates/hiveweb/src/models/role.rs`
- [x] T059 [P] [US3] Create permission check middleware in `crates/hiveweb/src/middleware/auth.rs`
- [x] T060 [US3] Add role-based route protection in backend API handlers
- [x] T061 [US3] Create permission guard component in `web/src/components/PermissionGuard.tsx`
- [x] T062 [US3] Implement role-based menu filtering in `web/src/components/Layout.tsx`
- [x] T063 [US3] Add role check in useAuth hook in `web/src/hooks/useAuth.ts`
- [x] T064 [US3] Implement redirect on insufficient permission in `web/src/pages/DashboardPage.tsx`
- [x] T065 [US3] Add visual indicators for disabled actions (delete button for non-super-admin)

**Checkpoint**: All three user stories should now work with proper role isolation

---

## Phase 6: User Story 4 - 仪表盘概览 (Priority: P2)

**Goal**: 管理员登录后看到系统概览信息

**Independent Test**: 可以独立测试仪表盘的数据展示和统计准确性

### Implementation for User Story 4

- [x] T066 [P] [US4] Create DashboardStatsResponse DTO in `crates/hiveweb/src/api/dashboard.rs`
- [x] T067 [P] [US4] Create get stats endpoint in `crates/hiveweb/src/api/dashboard.rs`
- [x] T068 [P] [US4] Create get recent logins endpoint in `crates/hiveweb/src/api/dashboard.rs`
- [x] T069 [US4] Implement dashboard service in `crates/hiveweb/src/services/dashboard.rs`
- [x] T070 [US4] Query total admins count in `crates/hiveweb/src/services/dashboard.rs`
- [x] T071 [US4] Query online admins (logged in 24h) in `crates/hiveweb/src/services/dashboard.rs`
- [x] T072 [US4] Query today login count in `crates/hiveweb/src/services/dashboard.rs`
- [x] T073 [US4] Query recent login records in `crates/hiveweb/src/services/dashboard.rs`
- [x] T074 [P] [US4] Create Dashboard component in `web/src/components/Dashboard.tsx`
- [x] T075 [P] [US4] Create DashboardPage in `web/src/pages/DashboardPage.tsx`
- [x] T076 [US4] Create stats cards UI in Dashboard component
- [x] T077 [US4] Create recent logins table in Dashboard component
- [x] T078 [US4] Add dashboard API calls in `web/src/services/dashboard.ts`
- [x] T079 [US4] Implement auto-refresh for dashboard data (optional)

**Checkpoint**: All user stories should now be independently functional

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Improvements that affect multiple user stories

- [x] T080 [P] Create database seeder script for initial super admin
- [x] T081 [P] Write migration rollback scripts
- [x] T082 [P] Add comprehensive error handling across all APIs
- [x] T083 [P] Add structured logging with tracing crate
- [x] T084 [P] Configure CORS for frontend-backend communication
- [x] T085 [P] Add rate limiting middleware
- [x] T086 [P] Create `.env.example` files for backend and frontend
- [x] T087 [P] Write deployment documentation (`specs/003-admin-center/DEPLOYMENT.md`)
- [x] T088 [P] Create admin user guide (`specs/003-admin-center/USER_GUIDE.md`)
- [x] T089 [P] Code cleanup and refactoring
- [x] T090 [P] Performance optimization (database query optimization, Redis caching)
- [x] T091 [P] Security hardening (input validation, SQL injection prevention)
- [ ] T092 [P] [FR-021] 添加 request-ID 中间件 + JSON 日志格式 + 手机号遮码工具，位于 `crates/hiveweb/src/middleware/request_id.rs` 与 `crates/hiveweb/src/utils/logging.rs`
- [ ] T093 [P] [FR-022] 添加审计日志写入（管理员 CRUD 事件）至 `crates/hiveweb/src/services/audit.rs`
- [ ] T094 [P] [FR-020 / SC-008] 集成 axe-core 自动化检测进入 Vitest 套件，覆盖 LoginPage、AdminPage、DashboardPage
- [ ] T095 [P] [FR-020] 为所有交互组件补充 ARIA 标签与键盘焦点顺序审查
- [ ] T096 [FR-022] 编写迁移 V004：login_records 改 ON DELETE SET NULL，并增加 admin_phone_snapshot / admin_nickname_snapshot 列；更新 LoginRecord 模型与 dashboard 查询
- [ ] T097 [FR-022] 编写迁移 V005：login_records.idx_login_at 改为 DESC 索引以加速仪表盘 `ORDER BY login_at DESC LIMIT 10` 查询
- [ ] T098 [P] [SC-002] 性能基准：使用 `seed` 工具生成 100 个管理员账号，运行 `wrk` 或 criterion 压测 `GET /api/admins?page=1&page_size=10`，确认 p95 ≤ 2000 ms；脚本与基准结果记录在 `crates/hiveweb/benches/admin_list.rs`
- [ ] T099 [P] [SC-003] 性能基准：对 `GET /api/dashboard/stats` 与 `GET /api/dashboard/recent-logins` 运行同样压测，确认整体页面数据加载 ≤ 3 秒；记录在 `crates/hiveweb/benches/dashboard.rs`
- [ ] T100 [P] [Principle IV] 对 `admins`/`login_records` 表的所有热路径查询执行 `EXPLAIN`，确认使用 `idx_phone`、`idx_login_at`；结果写入 `specs/003-admin-center/perf-evidence.md`
- [ ] T101 [P] [SC-005] 使用 `seed` 工具构造 100 管理员 + 10 000 登录记录数据集，验证仪表盘与列表 p95 仍在预算内
- [x] T102 [spec.md §Error Codes] 统一后端业务错误码到 1001/1002/1003/1004/2001/3001/3002/3003/3004（修订 `crates/hiveweb/src/utils/error.rs`、`api/auth.rs`、`api/admin.rs`、`middleware/auth.rs`）

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies - can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion - BLOCKS all user stories
- **User Stories (Phase 3-6)**: All depend on Foundational phase completion
  - User stories can then proceed in parallel (if staffed)
  - Or sequentially in priority order (P1 → P2)
- **Polish (Phase 7)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational (Phase 2) - No dependencies on other stories
- **User Story 2 (P1)**: Can start after Foundational (Phase 2) - Independent from US1
- **User Story 3 (P2)**: Can start after Foundational (Phase 2) - Builds on US1 auth
- **User Story 4 (P2)**: Can start after Foundational (Phase 2) - Independent from other stories

### Within Each User Story

- Models before services
- Services before endpoints
- Backend before frontend integration
- Core implementation before integration

### Parallel Opportunities

- All Setup tasks marked [P] can run in parallel
- All Foundational tasks marked [P] can run in parallel
- Once Foundational phase completes, all user stories can start in parallel (if team capacity allows)
- Different user stories can be worked on in parallel by different team members
- Within each story, tasks marked [P] can run in parallel

---

## Parallel Example: User Story 1

```bash
# Launch all models for User Story 1 together:
Task: "T027 [P] [US1] Create LoginRequest/ LoginResponse DTOs in crates/hiveweb/src/api/auth.rs"
Task: "T034 [P] [US1] Create LoginForm component in web/src/components/LoginForm.tsx"

# Launch all backend services together:
Task: "T028 [US1] Implement login handler in crates/hiveweb/src/api/auth.rs"
Task: "T031 [US1] Implement login failure tracking with Redis in crates/hiveweb/src/services/auth.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL - blocks all stories)
3. Complete Phase 3: User Story 1
4. **STOP and VALIDATE**: Test User Story 1 independently
5. Deploy/demo if ready

### Incremental Delivery

1. Complete Setup + Foundational → Foundation ready
2. Add User Story 1 → Test independently → Deploy/Demo (MVP!)
3. Add User Story 2 → Test independently → Deploy/Demo
4. Add User Story 3 → Test independently → Deploy/Demo
5. Add User Story 4 → Test independently → Deploy/Demo
6. Each story adds value without breaking previous stories

### Parallel Team Strategy

With multiple developers:

1. Team completes Setup + Foundational together
2. Once Foundational is done:
   - Developer A: User Story 1
   - Developer B: User Story 2
   - Developer C: User Story 3
3. Stories complete and integrate independently

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- Avoid: vague tasks, same file conflicts, cross-story dependencies that break independence

## Task Summary

- **Total Tasks**: 114（原 91 项已完成 + 23 项新增待完成）
- **Completed**: 91 tasks
- **Pending**:
  - Phase 2.5 Tests (Red Phase): 13 项（T026a–T026m）— **必须先红灯**
  - Phase 7 Polish 补充: 10 项（T092–T101，含 a11y / 可观测性 / 审计 / 性能基准 / 索引验证）
- **Setup (Phase 1)**: 9/9 tasks ✅ (infra provisioned via `scripts/docker-compose.yml`)
- **Foundational (Phase 2)**: 17/17 tasks ✅
- **Tests (Phase 2.5)**: 0/13 tasks ⛔ 阻塞后续实现（宪法 Principle II 强制）
- **User Story 1 (Phase 3)**: 13/13 tasks ✅
- **User Story 2 (Phase 4)**: 18/18 tasks ✅
- **User Story 3 (Phase 5)**: 8/8 tasks ✅
- **User Story 4 (Phase 6)**: 14/14 tasks ✅
- **Polish (Phase 7)**: 12/22 tasks（T092–T101 待完成）

**Parallel Opportunities**: 45+ tasks marked with [P] can run in parallel
**Independent MVP**: User Story 1 (13 tasks) after Foundational phase
