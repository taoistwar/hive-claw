# Tasks: HiveGUI 独立桌面管理工具

**Input**: Design documents from `/specs/011-hivegui-standalone-mode/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, quickstart.md

**Tests**: 遵循宪法原则 II（Test-First Development），每个 Phase 包含对应测试任务。

**Organization**: Tasks grouped by user story. US1-US4 已完成（现有代码），US5-US13 为新增实体管理。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: 创建共享数据层和导航框架

- [x] T001 Create `crates/hivegui/src/datasource/entity_store.rs` with SQLite table creation for all 9 new entities (tags, categories, capabilities, plugins, functions, workflows, tools, skills, agents) per data-model.md
- [x] T002 Register entity_store module in `crates/hivegui/src/datasource/mod.rs`
- [x] T003 Initialize entity_store in Store::new() in `crates/hivegui/src/datasource/store.rs` (call entity_store::init_tables)
- [x] T003a Implement SQLite integrity check on startup in `crates/hivegui/src/datasource/store.rs` — execute `PRAGMA integrity_check` during Store::new(), if check fails log error and provide recovery option (recreate database), display user-friendly error message via gpui dialog (covers FR-006)

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: 扩展路由和导航，为所有用户故事提供 UI 框架

- [x] T004 Add 9 new routes to AppRoute enum in `crates/hivegui/src/ui/app.rs` (Tag, Category, Capability, Plugin, Function, Workflow, Tool, Skill, Agent)
- [x] T005 Add 9 new nav buttons to SidebarNav in `crates/hivegui/src/ui/sidebar_nav.rs` (标签, 分类, 能力, 插件, 函数, 工作流, 工具, 技能, Agent)
- [x] T006 Add route rendering for 9 new views in RootView in `crates/hivegui/src/ui/app.rs` (placeholder views initially)

**Checkpoint**: 路由和导航就绪，数据库表已创建

---

## Phase 3: User Story 5 - 标签管理 (Priority: P1)

**Goal**: 标签（Tag）的增删改查管理

**Independent Test**: 添加标签 → 编辑颜色 → 删除 → 重启验证持久化

### Implementation

- [x] T007 [US5] Implement Tag CRUD methods (list with pagination/search, get, create, update, delete) in `crates/hivegui/src/datasource/entity_store.rs`
- [x] T008 [US5] Create TagListView and TagFormView in `crates/hivegui/src/ui/tag_view.rs`
- [x] T009 [US5] Wire TagView into RootView route rendering in `crates/hivegui/src/ui/app.rs`

### Tests

- [x] T007t [US5] Add unit tests for Tag CRUD in `crates/hivegui/src/datasource/entity_store.rs`:
  - test_tag_create_and_get: 创建后可查询
  - test_tag_list_pagination: 分页正确
  - test_tag_search: 按名称搜索
  - test_tag_update: 更新后验证
  - test_tag_delete: 删除后不存在
  - test_tag_name_unique: 重复 name 报错

**Checkpoint**: 标签管理功能完整可用，单元测试通过

---

## Phase 4: User Story 6 - 分类管理 (Priority: P1)

**Goal**: 分类（Category）的增删改查管理，支持树形层级

**Independent Test**: 添加父分类 → 添加子分类 → 编辑 → 删除子分类 → 删除父分类 → 重启验证持久化

### Implementation

- [x] T010 [US6] Implement Category CRUD methods in `crates/hivegui/src/datasource/entity_store.rs`:
  - list with tree structure, get, create, update
  - delete: 执行前先查询 `SELECT COUNT(*) FROM categories WHERE parent_id = ?`
    - 若 count > 0，返回错误 `Err("该分类下有 {count} 个子分类，请先删除子分类")`
    - 若 count = 0，执行 DELETE
  - 删除 Category 时，将引用该 category 的实体的 category_id 置为 NULL:
    `UPDATE capabilities SET category_id = NULL WHERE category_id = ?`
- [x] T011 [US6] Create CategoryListView (tree view with indentation) and CategoryFormView (with parent selector) in `crates/hivegui/src/ui/category_view.rs`
- [x] T012 [US6] Wire CategoryView into RootView route rendering in `crates/hivegui/src/ui/app.rs`

### Tests

- [x] T010t [US6] Add unit tests for Category CRUD in `crates/hivegui/src/datasource/entity_store.rs`:
  - test_category_create_and_get: 创建后可查询
  - test_category_tree: 父子层级查询正确
  - test_category_delete_with_children: 有子分类时阻止删除
  - test_category_delete_cascade_null: 删除后引用实体的 category_id 置 NULL
  - test_category_slug_unique: slug 唯一约束
  - test_category_update: 更新后验证

**Checkpoint**: 分类管理功能完整可用，树形视图渲染正确，子分类保护测试通过

---

## Phase 5: User Story 7 - 能力管理 (Priority: P1)

**Goal**: 能力（Capability）的增删改查管理

**Independent Test**: 添加能力 → 编辑 → 删除 → 重启验证持久化

### Implementation

- [x] T013 [US7] Implement Capability CRUD methods (list with pagination/search, get, create, update, delete; name as PK) in `crates/hivegui/src/datasource/entity_store.rs`
- [x] T014 [US7] Create CapabilityListView and CapabilityFormView in `crates/hivegui/src/ui/capability_view.rs`
- [x] T015 [US7] Wire CapabilityView into RootView route rendering in `crates/hivegui/src/ui/app.rs`

### Tests

- [x] T013t [US7] Add unit tests for Capability CRUD in `crates/hivegui/src/datasource/entity_store.rs`:
  - test_capability_create_and_get: 创建后可查询
  - test_capability_name_pk: name 作为主键不可重复
  - test_capability_list_pagination: 分页正确
  - test_capability_search: 按名称搜索
  - test_capability_update: 更新后验证
  - test_capability_delete: 删除后不存在

**Checkpoint**: 能力管理功能完整可用，单元测试通过

---

## Phase 6: User Story 8 - 插件管理 (Priority: P1)

**Goal**: 插件（Plugin）的增删改查管理，identifier 唯一，支持软删除

**Independent Test**: 添加插件 → 编辑 → 删除 → 重启验证持久化

### Implementation

- [x] T016 [US8] Implement Plugin CRUD methods (list with pagination/search, soft delete via deleted_at, get, create, update; identifier UNIQUE check) in `crates/hivegui/src/datasource/entity_store.rs`
- [x] T017 [US8] Create PluginListView and PluginFormView (all hiveweb fields) in `crates/hivegui/src/ui/plugin_view.rs`
- [x] T018 [US8] Wire PluginView into RootView route rendering in `crates/hivegui/src/ui/app.rs`

### Tests

- [x] T016t [US8] Add unit tests for Plugin CRUD in `crates/hivegui/src/datasource/entity_store.rs`:
  - test_plugin_create_and_get: 创建后可查询
  - test_plugin_identifier_unique: identifier 唯一约束
  - test_plugin_soft_delete: 软删除后列表过滤，记录仍存在
  - test_plugin_list_pagination: 分页正确
  - test_plugin_search: 按名称搜索
  - test_plugin_update: 更新后验证

**Checkpoint**: 插件管理功能完整可用，identifier 唯一性验证通过，软删除测试通过

---

## Phase 7: User Story 9 - 函数管理 (Priority: P1)

**Goal**: 函数（Function）的增删改查管理，identifier 唯一

**Independent Test**: 添加函数 → 编辑 → 删除 → 重启验证持久化

### Implementation

- [x] T019 [US9] Implement Function CRUD methods (list with pagination/search, get, create, update, delete; identifier UNIQUE check) in `crates/hivegui/src/datasource/entity_store.rs`
- [x] T020 [US9] Create FunctionListView and FunctionFormView (all hiveweb fields including JSON schemas) in `crates/hivegui/src/ui/function_view.rs`
- [x] T021 [US9] Wire FunctionView into RootView route rendering in `crates/hivegui/src/ui/app.rs`

### Tests

- [x] T019t [US9] Add unit tests for Function CRUD in `crates/hivegui/src/datasource/entity_store.rs`:
  - test_function_create_and_get: 创建后可查询
  - test_function_identifier_unique: identifier 唯一约束
  - test_function_list_pagination: 分页正确
  - test_function_search: 按名称搜索
  - test_function_update: 更新后验证
  - test_function_delete: 删除后不存在

**Checkpoint**: 函数管理功能完整可用，单元测试通过

---

## Phase 8: User Story 10 - 工作流管理 (Priority: P1)

**Goal**: 工作流（Workflow）主表的增删改查管理，identifier 唯一，不含节点/边

**Independent Test**: 添加工作流 → 编辑 → 删除 → 重启验证持久化

### Implementation

- [x] T022 [US10] Implement Workflow CRUD methods (list with pagination/search, get, create, update, delete; identifier UNIQUE check) in `crates/hivegui/src/datasource/entity_store.rs`
- [x] T023 [US10] Create WorkflowListView and WorkflowFormView (main table fields only) in `crates/hivegui/src/ui/workflow_view.rs`
- [x] T024 [US10] Wire WorkflowView into RootView route rendering in `crates/hivegui/src/ui/app.rs`

### Tests

- [x] T022t [US10] Add unit tests for Workflow CRUD in `crates/hivegui/src/datasource/entity_store.rs`:
  - test_workflow_create_and_get: 创建后可查询
  - test_workflow_identifier_unique: identifier 唯一约束
  - test_workflow_list_pagination: 分页正确
  - test_workflow_search: 按名称搜索
  - test_workflow_update: 更新后验证
  - test_workflow_delete: 删除后不存在

**Checkpoint**: 工作流管理功能完整可用，单元测试通过

---

## Phase 9: User Story 11 - 工具管理 (Priority: P1)

**Goal**: 工具（Tool）的增删改查管理，identifier 唯一，kind CHECK 约束

**Independent Test**: 添加工具 → 编辑 → 删除 → 重启验证持久化

### Implementation

- [x] T025 [US11] Implement Tool CRUD methods in `crates/hivegui/src/datasource/entity_store.rs`:
  - list with pagination/search, get, create, update, delete
  - identifier UNIQUE check
  - CHECK constraint 实现（双层保障）:
    1. SQLite 表定义: `CHECK ( (kind=1 AND function_id IS NOT NULL) OR (kind=2 AND workflow_id IS NOT NULL) )`
    2. 应用层验证: create/update 前检查
       - kind=1 且 function_id 为空 → `Err("kind=function 时 function_id 不能为空")`
       - kind=2 且 workflow_id 为空 → `Err("kind=workflow 时 workflow_id 不能为空")`
       - kind=1 时 workflow_id 应为 NULL; kind=2 时 function_id 应为 NULL
- [x] T026 [US11] Create ToolListView and ToolFormView (all hiveweb fields) in `crates/hivegui/src/ui/tool_view.rs`
- [x] T027 [US11] Wire ToolView into RootView route rendering in `crates/hivegui/src/ui/app.rs`

### Tests

- [x] T025t [US11] Add unit tests for Tool CRUD in `crates/hivegui/src/datasource/entity_store.rs`:
  - test_tool_create_and_get: 创建后可查询
  - test_tool_identifier_unique: identifier 唯一约束
  - test_tool_check_constraint_kind1: kind=1 时 function_id 不能为空
  - test_tool_check_constraint_kind2: kind=2 时 workflow_id 不能为空
  - test_tool_list_pagination: 分页正确
  - test_tool_update: 更新后验证
  - test_tool_delete: 删除后不存在

**Checkpoint**: 工具管理功能完整可用，CHECK 约束测试通过

---

## Phase 10: User Story 12 - 技能管理 (Priority: P1)

**Goal**: 技能（Skill）的增删改查管理，identifier 唯一

**Independent Test**: 添加技能 → 编辑 → 删除 → 重启验证持久化

### Implementation

- [x] T028 [US12] Implement Skill CRUD methods (list with pagination/search, get, create, update, delete; identifier UNIQUE check) in `crates/hivegui/src/datasource/entity_store.rs`
- [x] T029 [US12] Create SkillListView and SkillFormView (all hiveweb fields including frontmatter JSON, content markdown) in `crates/hivegui/src/ui/skill_view.rs`
- [x] T030 [US12] Wire SkillView into RootView route rendering in `crates/hivegui/src/ui/app.rs`

### Tests

- [x] T028t [US12] Add unit tests for Skill CRUD in `crates/hivegui/src/datasource/entity_store.rs`:
  - test_skill_create_and_get: 创建后可查询
  - test_skill_identifier_unique: identifier 唯一约束
  - test_skill_list_pagination: 分页正确
  - test_skill_search: 按名称搜索
  - test_skill_update: 更新后验证
  - test_skill_delete: 删除后不存在

**Checkpoint**: 技能管理功能完整可用，单元测试通过

---

## Phase 11: User Story 13 - Agent 管理 (Priority: P1)

**Goal**: Agent 的增删改查管理，identifier 唯一，parent_agent_id 自引用

**Independent Test**: 添加 Agent → 编辑 → 删除 → 重启验证持久化

### Implementation

- [x] T031 [US13] Implement Agent CRUD methods in `crates/hivegui/src/datasource/entity_store.rs`:
  - list with pagination/search, get, create, update, delete
  - identifier UNIQUE check
  - parent_agent_id 处理:
    - 创建/更新时，若设置了 parent_agent_id，需检测循环引用：
      循环查询 `SELECT parent_agent_id FROM agents WHERE id = ?`，沿 parent 链向上遍历
      若遍历过程中遇到当前 agent 的 id，则返回 `Err("不允许形成循环引用：Agent A → Agent B → Agent A")`
    - 删除 Agent 时，将子 Agent 的 parent_agent_id 置为 NULL:
      `UPDATE agents SET parent_agent_id = NULL WHERE parent_agent_id = ?`
- [x] T032 [US13] Create AgentListView and AgentFormView (all fields including parent_agent_id selector) in `crates/hivegui/src/ui/agent_view.rs`
- [x] T033 [US13] Wire AgentView into RootView route rendering in `crates/hivegui/src/ui/app.rs`

### Tests

- [x] T031t [US13] Add unit tests for Agent CRUD in `crates/hivegui/src/datasource/entity_store.rs`:
  - test_agent_create_and_get: 创建后可查询
  - test_agent_identifier_unique: identifier 唯一约束
  - test_agent_cycle_detection: 检测循环引用并拒绝保存
  - test_agent_delete_orphan_children: 删除 Agent 后子 Agent 的 parent_agent_id 置 NULL
  - test_agent_list_pagination: 分页正确
  - test_agent_search: 按名称/identifier 搜索
  - test_agent_update: 更新后验证

**Checkpoint**: Agent 管理功能完整可用，循环引用检测测试通过

---

## Phase 12: Polish & Cross-Cutting Concerns

**Purpose**: 跨实体通用改进

- [x] T034 Verify all 9 entity tables created correctly on app startup in `crates/hivegui/src/datasource/entity_store.rs`
- [x] T035 Ensure consistent error messages for identifier UNIQUE violations across all entities
- [x] T036 Ensure consistent pagination (20 per page) and search behavior across all 9 entity views
- [x] T037 Run `cargo build -p hivegui` and fix all compilation errors
- [x] T038 Run `cargo clippy -p hivegui` and fix all warnings
- [x] T039 Add structured logging to all entity CRUD methods in `crates/hivegui/src/datasource/entity_store.rs`:
  - Use `tracing::info!` for each operation with: entity type, operation name, outcome, duration
  - Example: `tracing::info!(entity = "tag", op = "create", name = %name, duration_ms = %elapsed, "tag created");`
  - Use `tracing::error!` for failures with error detail
  - Add `tracing` dependency to hivegui Cargo.toml if not present
- [x] T040 Integration test: 启动应用 → 添加 Tag → 添加 Category → 添加 Agent → 重启 → 验证数据持久化
- [x] T041 Implement consistent UI patterns across all 9 entity views:
  - Delete confirmation dialog: "确定要删除 [实体名称] 吗？此操作不可撤销。"
  - Empty state message: "暂无数据"
  - Search no result: "未找到匹配项"
  - Form error preservation: 提交失败时保留表单数据，不关闭表单
- [x] T042 Add field validation in entity_store.rs and UI forms per spec.md rules:
  - identifier: `^[a-zA-Z0-9_-]+$`, max 255 chars
  - slug: `^[a-z0-9-]+$`, max 255 chars
  - name: max 255 chars, not empty
  - description: max 2000 chars, optional
  - JSON fields: valid JSON format, max 1MB
  - color (Tag): HEX format `#FF5733`

---

## Phase 13: Advanced Features (FR-025/026/027)

**Purpose**: 实现高级功能需求

### FR-025: 错误恢复机制

- [x] T043 Implement automatic retry mechanism for temporary errors in `crates/hivegui/src/datasource/entity_store.rs`:
  - Retry up to 3 times with exponential backoff (1s, 2s, 4s)
  - Apply to: database lock errors, file busy errors
  - Add `retry_with_backoff` helper function
  - Log retry attempts with tracing
- [x] T044 Add manual recovery function in `crates/hivegui/src/datasource/entity_store.rs`:
  - `restore_from_backup(backup_path: &str)` function
  - Validate backup file format before restore
  - Create current database backup before restore
  - Return success/failure status

### FR-026: 数据导出/备份功能

- [x] T045 Implement data export function in `crates/hivegui/src/datasource/entity_store.rs`:
  - `export_all_data(output_path: &str)` function
  - Export all entities to JSON format
  - Include metadata: version, export_timestamp, entity_count
  - JSON schema: `{ "version": "1.0", "exported_at": "...", "entities": { "tags": [...], "categories": [...], ... } }`
- [x] T046 Implement data import function in `crates/hivegui/src/datasource/entity_store.rs`:
  - `import_from_backup(backup_path: &str)` function
  - Validate JSON schema and version compatibility
  - Handle conflicts: skip existing records or update based on updated_at
  - Create transaction for atomic import
- [x] T047 Add UI for export/import in `crates/hivegui/src/ui/settings_view.rs`:
  - "导出数据" button with file path input
  - "导入数据" button with file path input
  - Show progress and success/failure messages

### FR-027: 数据库 Schema 版本管理

- [x] T048 Create schema version table in `crates/hivegui/src/datasource/entity_store.rs`:
  - `schema_versions` table: version (INTEGER PK), applied_at (TEXT), description (TEXT)
  - Initialize with version 1.0 on first run
- [x] T049 Implement migration system in `crates/hivegui/src/datasource/entity_store.rs`:
  - `get_current_version()` function
  - `run_migrations()` function with version check
  - Migration scripts as Rust functions (not SQL files)
  - Rollback support: backup before migration, restore on failure
- [x] T050 Add startup version check in `crates/hivegui/src/datasource/store.rs`:
  - Check schema version on app startup
  - Auto-run migrations if version mismatch
  - Display migration progress in UI
  - Handle migration failures with user-friendly error

### Tests

- [x] T051 Add unit tests for FR-025/026/027 in `crates/hivegui/tests/integration_test.rs`:
  - test_retry_mechanism: verify retry on temporary errors
  - test_export_import: export data, import to new database, verify integrity
  - test_schema_version_management: create database, run migrations, verify version tracking
  - test_migration_rollback: verify migration idempotency and data preservation
  - test_import_invalid_file: verify validation of invalid import files
  - test_export_empty_database: verify export of empty database

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies - creates shared infrastructure
- **Foundational (Phase 2)**: Depends on Setup - adds routing/nav framework
- **User Stories (Phase 3-11)**: All depend on Setup + Foundational completion
  - US5 (Tag) and US6 (Category) should be done first as other entities reference Category
  - US8 (Plugin) should be done before US9 (Function) as Function references Plugin
  - US9 (Function) and US10 (Workflow) should be done before US11 (Tool) as Tool references both
- **Polish (Phase 12)**: Depends on all user stories being complete

### Recommended Execution Order

1. Phase 1 (Setup) → Phase 2 (Foundational)
2. US5 (Tag) → US6 (Category) — other entities reference Category
3. US7 (Capability) — independent
4. US8 (Plugin) → US9 (Function) — Function references Plugin
5. US10 (Workflow) → US11 (Tool) — Tool references Function + Workflow
6. US12 (Skill) — independent
7. US13 (Agent) — independent
8. Phase 12 (Polish)

### Parallel Opportunities

- US5 (Tag), US7 (Capability) can be parallelized after Phase 2
- US12 (Skill), US13 (Agent) can be parallelized after Phase 2
- Within each story: data layer (store) and UI layer (view) are sequential

---

## Implementation Strategy

### MVP First (US5-US6 Only)

1. Complete Phase 1-2: Setup + Foundational
2. Complete Phase 3-4: Tag + Category management
3. **STOP and VALIDATE**: Test Tag and Category independently
4. Verify CRUD pattern works before replicating

### Incremental Delivery

1. Setup + Foundational → Foundation ready
2. Tag + Category → Core entities ready
3. Capability + Plugin → More entities
4. Function + Workflow → Complex entities with JSON
5. Tool + Skill + Agent → Final entities
6. Polish → Production ready

---

## Notes

- US1-US4 (Home, DataSource, GlobalConfig, LLMConfig) are already implemented in existing code
- All 9 new entities follow the same CRUD pattern: list view (table + search + pagination) + form view (modal/panel)
- entity_store.rs is the single shared data layer file for all 9 entities
- Each entity gets its own UI view file (tag_view.rs, category_view.rs, etc.)
- Category tree view uses indentation (not complex tree component)
- Plugin uses soft delete (deleted_at field)
- Tool has CHECK constraint (kind=1 → function_id, kind=2 → workflow_id)
