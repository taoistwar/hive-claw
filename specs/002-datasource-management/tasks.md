# Tasks: 数据源管理

**Input**: Design documents from `/specs/002-datasource-management/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md

**Organization**: Tasks grouped by user story for independent implementation and testing
**Architecture Note**: 所有功能在 `hivegui` crate 内实现，不经过 HTTP API，直接本地调用

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization and Rust dependencies

- [ ] T001 Create `crates/hivegui/src/datasource/` module structure (mod.rs, models.rs, store.rs, mysql_client.rs, crypto.rs)
- [ ] T002 [P] Add dependencies to `crates/hivegui/Cargo.toml`: `mysql_async`, `sqlx` with sqlite feature, `chacha20poly1305`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core infrastructure that MUST be complete before ANY user story

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [ ] T003 Implement `DataSource` model in `crates/hivegui/src/datasource/models.rs` (id, name, host, port, username, encrypted_password, created_at, updated_at)
- [ ] T004 Implement `DatabaseInfo`, `TableInfo`, `ColumnInfo`, `TableData` models in `crates/hivegui/src/datasource/models.rs`
- [ ] T005 [P] Implement SQLite store in `crates/hivegui/src/datasource/store.rs` (CRUD operations for DataSource using sqlx, database at `~/.local/share/hivegui/datasources.db`)
- [ ] T006 [P] Implement password encryption in `crates/hivegui/src/datasource/crypto.rs` (chacha20poly1305 encrypt/decrypt)
- [ ] T007 [P] Implement MySQL client in `crates/hivegui/src/datasource/mysql_client.rs` (connect, test_connection, query_databases, query_tables, query_columns, query_ddl, query_data)
- [ ] T008 Implement `crates/hivegui/src/datasource/mod.rs` module exports

**Checkpoint**: Foundation ready - user story implementation can now begin

---

## Phase 3: User Story 1 - 添加MySQL数据源 (Priority: P1) 🎯 MVP

**Goal**: 用户可以添加、编辑、删除MySQL数据源，支持连接测试，数据源信息加密持久化存储

**Independent Test**: 添加数据源流程（输入→测试→保存→列表展示）可独立测试

### Implementation for User Story 1

- [ ] T009 [US1] Implement data source panel UI in `crates/hivegui/src/ui/datasource_panel.rs` (left column: list, add, edit, delete buttons)
- [ ] T010 [US1] Wire panel to datasource store for list/create/update/delete operations
- [ ] T011 [US1] Add connection test UI flow and error handling in datasource_panel.rs
- [ ] T012 [US1] Implement add/edit dialog with form validation (host, port, username, password, name)

**Checkpoint**: User can add, edit, delete data sources with connection testing

---

## Phase 4: User Story 2 - 浏览数据库和表 (Priority: P2)

**Goal**: 左侧树形导航展示数据源→数据库→表的三级结构

**Independent Test**: 树形导航展开/收起、数据库列表、表列表加载可独立测试

### Implementation for User Story 2

- [ ] T013 [US2] Implement tree navigation component in `crates/hivegui/src/ui/tree_nav.rs` (data source → database → table hierarchy)
- [ ] T014 [US2] Connect tree expand events to mysql_client for database/table listing
- [ ] T015 [US2] Handle loading states and empty results in tree navigation

**Checkpoint**: User can expand data source to see databases, expand database to see tables

---

## Phase 5: User Story 3 - 查看表结构信息 (Priority: P3)

**Goal**: 表查看器的"列"和"DDL"两个Tab展示表结构

**Independent Test**: 列信息和DDL展示可独立测试

### Implementation for User Story 3

- [ ] T016 [US3] Implement table viewer UI in `crates/hivegui/src/ui/table_viewer.rs` with tab component (列, DDL, 数据)
- [ ] T017 [US3] Connect "列" tab to mysql_client query_columns, display column info table
- [ ] T018 [US3] Connect "DDL" tab to mysql_client query_ddl, display with syntax highlighting
- [ ] T019 [US3] Wire tree_nav selection to table_viewer loading

**Checkpoint**: User can view table columns and DDL in right panel

---

## Phase 6: User Story 4 - 查看表数据 (Priority: P3)

**Goal**: 表查看器的"数据"Tab支持WHERE条件搜索、排序和分页

**Independent Test**: 数据预览、条件查询、分页可独立测试

### Implementation for User Story 4

- [ ] T020 [US4] Connect "数据" tab to mysql_client query_data with limit/offset/where/order_by parameters
- [ ] T021 [US4] Implement pagination controls in table viewer
- [ ] T022 [US4] Implement WHERE condition input and ORDER BY input in UI
- [ ] T023 [US4] Display data in tabular format with column headers

**Checkpoint**: User can browse table data with filtering, sorting, and pagination

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Error handling, edge cases, logging, observability

- [ ] T024 Add structured logging for all datasource operations (connect, query, errors)
- [ ] T025 Implement connection timeout handling (10s timeout per spec)
- [ ] T026 Handle edge cases: empty database, empty table, connection failure, special characters in names
- [ ] T027 Ensure password is never exposed in UI or logs
- [ ] T028 Update `crates/hivegui/src/lib.rs` to export datasource module

---

## Dependency Graph

```
Phase 1 (Setup)
    │
    ▼
Phase 2 (Foundational) ─────────────────────────────────────────┐
    │                                                            │
    ▼                                                            │
Phase 3 (US1) ──────────────────┐                               │
                                 │                               │
Phase 4 (US2) ──────────────── │ ──────────────────────────────┤
                                 │                               │
Phase 5 (US3) ──────────────── │ ──────────────────────────────┤
                                 │                               │
Phase 6 (US4) ──────────────── │ ──────────────────────────────┘
                                                             │
Phase 7 (Polish) ◀────────────────────────────────────────────┘
```

---

## Parallel Execution Opportunities

- **T002**: Independent Cargo.toml change
- **T005, T006, T007**: Can run in parallel (foundational modules independent)
- **T009, T010, T011, T012**: Can run in parallel within US1 UI work (different files)

---

## Implementation Strategy

1. **MVP (Phase 3)**: 只实现US1，数据源CRUD + UI面板
2. **Incremental delivery**: 每完成一个用户故事即可测试
3. **Service-first**: 先完成 datasource 模块的核心服务（store, mysql_client），再实现UI
4. **UI wiring last**: UI组件在对应服务完成之后连接

---

## Summary

| Metric | Value |
|--------|-------|
| Total tasks | 28 |
| Phase 1 (Setup) | 2 |
| Phase 2 (Foundational) | 6 |
| Phase 3 (US1 - MVP) | 4 |
| Phase 4 (US2) | 3 |
| Phase 5 (US3) | 4 |
| Phase 6 (US4) | 4 |
| Phase 7 (Polish) | 5 |
| Parallelizable tasks | 5 |
| Independent stories | 4 (after foundational complete) |

---

## Suggested MVP Scope

**Phase 1 + Phase 2 + Phase 3 (US1)** = 12 tasks

实现后可测试：添加数据源 → 连接测试 → 保存 → 列表展示 → 编辑 → 删除
