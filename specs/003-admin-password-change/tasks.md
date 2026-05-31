---

description: "Task list for admin password change feature"
---

# Tasks: 管理员修改密码

**Input**: Design documents from `/specs/003-admin-password-change/`
**Prerequisites**: spec.md (required), existing admin-center implementation

**Tests**: 测试为强制要求（宪法 Principle II – NON-NEGOLABLE）。必须先编写并红灯运行对应的契约测试，再进入实现阶段。

**Organization**: Tasks are grouped by user story to enable independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this belongs to (e.g., US1)
- Include exact file paths in descriptions

## Path Conventions

- **Backend (Rust)**: `crates/hiveweb-admin/src/`
- **Frontend (TypeScript/React)**: `web-admin/src/`
- **Shared**: Specs in `specs/003-admin-password-change/`

---

## Phase 1: Tests (Red Phase)

**Purpose**: 契约测试必须先于实现编写，按宪法 Principle II 执行 TDD（Red → Green → Refactor）。

**CRITICAL**: 这些测试必须先观察到失败，再开始对应的实现任务。

### Contract tests

- [x] T103 [P] [US1] Contract test: `POST /api/auth/change-password` 成功/旧密码错误/新密码不合法 三类响应在 `crates/hiveweb/tests/contract_password.rs`（3/3 通过）

### Integration tests

- [x] T104 [P] [US1] Integration test: 密码修改后使用新密码登录成功（FR-105）在 `crates/hiveweb/tests/it_password_change.rs`（2/2 通过）
- [x] T105 [P] [US1] Integration test: 旧密码错误 5 次后锁定（FR-107）在 `crates/hiveweb/tests/it_password_lockout.rs`（1/1 通过）

**Checkpoint**: Phase 1 完成 - 测试编写并观察到红灯

---

## Phase 2: Backend Implementation

**Goal**: 实现后端密码修改 API

### Implementation

- [x] T106 [P] [US1] Add ChangePasswordRequest DTO in `crates/hiveweb-admin/src/api/auth.rs`
- [x] T107 [US1] Implement change password handler in `crates/hiveweb-admin/src/api/auth.rs`
- [x] T108 [US1] Implement password validation (new != old, format check) in `crates/hiveweb-admin/src/utils/password.rs`
- [x] T109 [US1] Implement password update service in `crates/hiveweb-admin/src/services/auth.rs`
- [x] T110 [US1] Add error code 3008 (NEW_PASSWORD_SAME_AS_OLD) in `crates/hiveweb-admin/src/utils/error.rs`
- [x] T111 [US1] Register route `POST /api/auth/change-password` in `crates/hiveweb-admin/src/api/mod.rs`
- [x] T112 [US1] Add audit log for password change in `crates/hiveweb-admin/src/services/audit.rs`

**Checkpoint**: Backend tests should turn green

---

## Phase 3: Frontend Implementation

**Goal**: 实现前端密码修改 UI

### Implementation

- [x] T113 [P] [US1] Create ChangePasswordForm component in `web-admin/src/components/ChangePasswordForm.tsx`
- [x] T114 [P] [US1] Create ChangePasswordPage in `web-admin/src/pages/ChangePasswordPage.tsx`
- [x] T115 [US1] Add change password API call in `web-admin/src/services/auth.ts`
- [x] T116 [US1] Add route for /settings/change-password in `web-admin/src/App.tsx`
- [x] T117 [US1] Add "修改密码" menu item in Layout component in `web-admin/src/components/Layout.tsx`
- [x] T118 [US1] Implement password strength indicator in ChangePasswordForm

**Checkpoint**: Frontend password change UI functional

---

## Dependencies & Execution Order

### Phase Dependencies

- **Tests (Phase 1)**: No dependencies - can start immediately
- **Backend (Phase 2)**: Depends on Phase 1 tests written
- **Frontend (Phase 3)**: Depends on Phase 2 backend API

### Within Each Phase

- DTOs before handlers
- Handlers before routes
- Backend before frontend

### Parallel Opportunities

- T103/T104/T105 can run in parallel (different test files)
- T106 can run in parallel with frontend setup
- T113/T114 can run in parallel

---

## Implementation Strategy

### MVP First

1. Complete Phase 1: Tests (Red)
2. Complete Phase 2: Backend (Green)
3. Complete Phase 3: Frontend
4. **STOP and VALIDATE**: Test password change independently

### Incremental Delivery

1. Backend API first → Test independently
2. Frontend UI → Test independently
3. Each adds value without breaking existing features

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Commit after each task or logical group
- Avoid: vague tasks, same file conflicts, cross-story dependencies

## Task Summary

- **Total Tasks**: 16
- **Tests (Phase 1)**: 3 tasks
- **Backend (Phase 2)**: 7 tasks
- **Frontend (Phase 3)**: 6 tasks
