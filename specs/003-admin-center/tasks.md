---

description: "Task list for admin center implementation"
---

# Tasks: 管理中心

**Input**: Design documents from `/specs/003-admin-center/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Tests**: Tests are OPTIONAL for this project. This task list does NOT include test tasks by default.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
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

- [ ] T001 [P] Create `crates/hiveweb/` directory structure per plan.md
- [ ] T002 [P] Create `web/` directory structure per plan.md
- [ ] T003 [P] Initialize Rust workspace in `crates/hiveweb/Cargo.toml`
- [ ] T004 [P] Initialize npm package in `web/package.json`
- [ ] T005 [P] Configure Rust linting: `cargo fmt` and `cargo clippy`
- [ ] T006 [P] Configure TypeScript linting: ESLint + Prettier
- [ ] T007 [P] Setup MySQL database: `CREATE DATABASE hiveweb`
- [ ] T008 [P] Setup Redis: install and configure connection
- [ ] T009 [P] Setup Rustfs/S3: configure local MinIO or S3 endpoint

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core infrastructure that MUST be complete before ANY user story can be implemented

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [ ] T010 [P] Create database migration framework using SQLx
- [ ] T011 Create `admins` table migration (V001__create_admins_table.sql)
- [ ] T012 Create `login_records` table migration (V002__create_login_records_table.sql)
- [ ] T013 [P] Implement MySQL connection pool in `crates/hiveweb/src/db/connection.rs`
- [ ] T014 [P] Implement Redis connection pool in `crates/hiveweb/src/cache/redis.rs`
- [ ] T015 [P] Implement Rustfs/S3 client in `crates/hiveweb/src/storage/s3.rs`
- [ ] T016 [P] Implement JWT authentication middleware in `crates/hiveweb/src/middleware/auth.rs`
- [ ] T017 [P] Create Admin model in `crates/hiveweb/src/models/admin.rs`
- [ ] T018 [P] Create LoginRecord model in `crates/hiveweb/src/models/login_record.rs`
- [ ] T019 [P] Create Role enum and permissions in `crates/hiveweb/src/models/role.rs`
- [ ] T020 [P] Implement password hashing with bcrypt in `crates/hiveweb/src/utils/password.rs`
- [ ] T021 [P] Create base API response types in `crates/hiveweb/src/api/response.rs`
- [ ] T022 [P] Setup axum router in `crates/hiveweb/src/api/mod.rs`
- [ ] T023 [P] Create frontend API service in `web/src/services/api.ts`
- [ ] T024 [P] Create frontend auth service in `web/src/services/auth.ts`
- [ ] T025 [P] Create React Router setup in `web/src/App.tsx`
- [ ] T026 [P] Create Layout component in `web/src/components/Layout.tsx`

**Checkpoint**: Foundation ready - user story implementation can now begin in parallel

---

## Phase 3: User Story 1 - 管理员登录 (Priority: P1) 🎯 MVP

**Goal**: 管理员可以使用手机号和密码登录系统，登录成功后更新最后登录时间

**Independent Test**: 可以独立测试登录流程，包括验证、会话创建、最后登录时间更新

### Implementation for User Story 1

- [ ] T027 [P] [US1] Create LoginRequest/ LoginResponse DTOs in `crates/hiveweb/src/api/auth.rs`
- [ ] T028 [US1] Implement login handler in `crates/hiveweb/src/api/auth.rs`
- [ ] T029 [US1] Implement password validation in `crates/hiveweb/src/utils/password.rs`
- [ ] T030 [US1] Implement JWT token generation in `crates/hiveweb/src/middleware/auth.rs`
- [ ] T031 [US1] Implement login failure tracking with Redis in `crates/hiveweb/src/services/auth.rs`
- [ ] T032 [US1] Update last_login_at on successful login in `crates/hiveweb/src/models/admin.rs`
- [ ] T033 [US1] Implement account lock/unlock logic in `crates/hiveweb/src/services/auth.rs`
- [ ] T034 [US1] Create LoginForm component in `web/src/components/LoginForm.tsx`
- [ ] T035 [US1] Create LoginPage in `web/src/pages/LoginPage.tsx`
- [ ] T036 [US1] Implement useAuth hook in `web/src/hooks/useAuth.ts`
- [ ] T037 [US1] Add login API call in `web/src/services/auth.ts`
- [ ] T038 [US1] Store JWT token in localStorage/cookie in `web/src/utils/auth.ts`
- [ ] T039 [US1] Implement auto-redirect after login in `web/src/pages/LoginPage.tsx`

**Checkpoint**: At this point, User Story 1 should be fully functional and testable independently

---

## Phase 4: User Story 2 - 管理员账号管理 (Priority: P1)

**Goal**: 系统管理员和超级管理员可以添加、删除、修改、禁用/启用管理员账号

**Independent Test**: 可以独立测试管理员的增删改查操作

### Implementation for User Story 2

- [ ] T040 [P] [US2] Create AdminRequest/AdminResponse DTOs in `crates/hiveweb/src/api/admin.rs`
- [ ] T041 [P] [US2] Create admin list API endpoint in `crates/hiveweb/src/api/admin.rs`
- [ ] T042 [P] [US2] Create get admin by ID endpoint in `crates/hiveweb/src/api/admin.rs`
- [ ] T043 [P] [US2] Create create admin endpoint in `crates/hiveweb/src/api/admin.rs`
- [ ] T044 [P] [US2] Create update admin endpoint in `crates/hiveweb/src/api/admin.rs`
- [ ] T045 [P] [US2] Create delete admin endpoint in `crates/hiveweb/src/api/admin.rs`
- [ ] T046 [P] [US2] Create toggle admin status endpoint in `crates/hiveweb/src/api/admin.rs`
- [ ] T047 [US2] Implement admin service layer in `crates/hiveweb/src/services/admin_service.rs`
- [ ] T048 [US2] Add phone uniqueness validation in `crates/hiveweb/src/services/admin_service.rs`
- [ ] T049 [US2] Implement "cannot delete super admin" check in `crates/hiveweb/src/services/admin_service.rs`
- [ ] T050 [US2] Implement "cannot disable last super admin" check in `crates/hiveweb/src/services/admin_service.rs`
- [ ] T051 [P] [US2] Create AdminTable component in `web/src/components/AdminTable.tsx`
- [ ] T052 [P] [US2] Create AdminForm component in `web/src/components/AdminForm.tsx`
- [ ] T053 [US2] Create AdminPage in `web/src/pages/AdminPage.tsx`
- [ ] T054 [US2] Implement useAdmin hook in `web/src/hooks/useAdmin.ts`
- [ ] T055 [US2] Add admin CRUD API calls in `web/src/services/admin.ts`
- [ ] T056 [US2] Implement pagination in AdminTable component
- [ ] T057 [US2] Add confirm dialogs for delete/disable actions

**Checkpoint**: At this point, User Stories 1 AND 2 should both work independently

---

## Phase 5: User Story 3 - 管理员角色权限管理 (Priority: P2)

**Goal**: 不同角色的管理员登录后看到不同的功能菜单和操作权限

**Independent Test**: 可以独立测试不同角色的权限边界

### Implementation for User Story 3

- [ ] T058 [P] [US3] Define role permissions map in `crates/hiveweb/src/models/role.rs`
- [ ] T059 [P] [US3] Create permission check middleware in `crates/hiveweb/src/middleware/auth.rs`
- [ ] T060 [US3] Add role-based route protection in backend API handlers
- [ ] T061 [US3] Create permission guard component in `web/src/components/PermissionGuard.tsx`
- [ ] T062 [US3] Implement role-based menu filtering in `web/src/components/Layout.tsx`
- [ ] T063 [US3] Add role check in useAuth hook in `web/src/hooks/useAuth.ts`
- [ ] T064 [US3] Implement redirect on insufficient permission in `web/src/pages/DashboardPage.tsx`
- [ ] T065 [US3] Add visual indicators for disabled actions (delete button for non-super-admin)

**Checkpoint**: All three user stories should now work with proper role isolation

---

## Phase 6: User Story 4 - 仪表盘概览 (Priority: P2)

**Goal**: 管理员登录后看到系统概览信息

**Independent Test**: 可以独立测试仪表盘的数据展示和统计准确性

### Implementation for User Story 4

- [ ] T066 [P] [US4] Create DashboardStatsResponse DTO in `crates/hiveweb/src/api/dashboard.rs`
- [ ] T067 [P] [US4] Create get stats endpoint in `crates/hiveweb/src/api/dashboard.rs`
- [ ] T068 [P] [US4] Create get recent logins endpoint in `crates/hiveweb/src/api/dashboard.rs`
- [ ] T069 [US4] Implement dashboard service in `crates/hiveweb/src/services/dashboard.rs`
- [ ] T070 [US4] Query total admins count in `crates/hiveweb/src/services/dashboard.rs`
- [ ] T071 [US4] Query online admins (logged in 24h) in `crates/hiveweb/src/services/dashboard.rs`
- [ ] T072 [US4] Query today login count in `crates/hiveweb/src/services/dashboard.rs`
- [ ] T073 [US4] Query recent login records in `crates/hiveweb/src/services/dashboard.rs`
- [ ] T074 [P] [US4] Create Dashboard component in `web/src/components/Dashboard.tsx`
- [ ] T075 [P] [US4] Create DashboardPage in `web/src/pages/DashboardPage.tsx`
- [ ] T076 [US4] Create stats cards UI in Dashboard component
- [ ] T077 [US4] Create recent logins table in Dashboard component
- [ ] T078 [US4] Add dashboard API calls in `web/src/services/dashboard.ts`
- [ ] T079 [US4] Implement auto-refresh for dashboard data (optional)

**Checkpoint**: All user stories should now be independently functional

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Improvements that affect multiple user stories

- [ ] T080 [P] Create database seeder script for initial super admin
- [ ] T081 [P] Write migration rollback scripts
- [ ] T082 [P] Add comprehensive error handling across all APIs
- [ ] T083 [P] Add structured logging with tracing crate
- [ ] T084 [P] Configure CORS for frontend-backend communication
- [ ] T085 [P] Add rate limiting middleware
- [ ] T086 [P] Create `.env.example` files for backend and frontend
- [ ] T087 [P] Write deployment documentation
- [ ] T088 [P] Create admin user guide
- [ ] T089 [P] Code cleanup and refactoring
- [ ] T090 [P] Performance optimization (database query optimization, Redis caching)
- [ ] T091 [P] Security hardening (input validation, SQL injection prevention)

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

- **Total Tasks**: 91
- **Setup (Phase 1)**: 9 tasks
- **Foundational (Phase 2)**: 17 tasks
- **User Story 1 (Phase 3)**: 13 tasks
- **User Story 2 (Phase 4)**: 18 tasks
- **User Story 3 (Phase 5)**: 8 tasks
- **User Story 4 (Phase 6)**: 14 tasks
- **Polish (Phase 7)**: 12 tasks

**Parallel Opportunities**: 45+ tasks marked with [P] can run in parallel
**Independent MVP**: User Story 1 (13 tasks) after Foundational phase
