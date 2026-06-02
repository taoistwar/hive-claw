# Tasks: 游戏别名管理

**Input**: Design documents from `specs/006-game-alias-management/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/api.md

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Database migration files and module registration

- [x] T001 Create migration V045__create_games_table.sql in `crates/hiveweb/migrations/` per data-model.md DDL (games table: id BIGINT PK AUTO_INCREMENT, name VARCHAR(50) NOT NULL UNIQUE, created_at/updated_at DATETIME, INDEX idx_name)
- [x] T002 [P] Create migration V046__create_game_alias_entries_table.sql in `crates/hiveweb/migrations/` per data-model.md DDL (game_alias_entries table: id BIGINT PK AUTO_INCREMENT, game_id BIGINT NOT NULL, alias VARCHAR(50) NOT NULL UNIQUE, INDEX idx_game_id, INDEX idx_alias, FK → games(id) ON DELETE CASCADE)
- [x] T003 [P] Register game module in `crates/hiveweb/src/models/mod.rs` (`pub mod game;`)
- [x] T004 [P] Register game module in `crates/hiveweb/src/services/mod.rs` (`pub mod game_service;`)
- [x] T005 [P] Register game module in `crates/hiveweb/src/api/mod.rs` (`pub mod game;` and `.merge(game::router())` in admin_protected_routes)

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core backend models, DTOs, service infrastructure, and error codes that ALL user stories depend on

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T006 [P] Add game alias error codes (4001-4008) to `crates/hiveweb/src/utils/error.rs` (GAME_ALIAS_NOT_FOUND, GAME_NAME_ALREADY_EXISTS, GAME_NAME_EMPTY, GAME_NAME_TOO_LONG, ALIASES_EMPTY, ALIAS_TOO_LONG, ALIASES_TOO_MANY, ALIAS_ALREADY_IN_USE)
- [x] T007 [P] Add audit operation types (GAME_ALIAS_CREATE, GAME_ALIAS_UPDATE, GAME_ALIAS_DELETE) to existing `Operation` enum or create new enum variant in `crates/hiveweb/src/services/audit.rs`
- [x] T008 Create Game model struct in `crates/hiveweb/src/models/game.rs` (id: i64, name: String, aliases: Vec<String> via FromRow+JSON_ARRAYAGG, created_at/updated_at: chrono::DateTime<Utc>, derive Debug/Clone/Serialize/Deserialize/FromRow)
- [x] T009 Create Game DTOs in `crates/hiveweb/src/models/game.rs` (CreateGameRequest, UpdateGameRequest, GameResponse, GameListResponse with Serialize/Deserialize, validation constants: MAX_NAME_LENGTH=50, MAX_ALIAS_LENGTH=50, MAX_ALIASES_COUNT=20, DEFAULT_PAGE_SIZE=10, MAX_PAGE_SIZE=100)

**Checkpoint**: Foundation ready - user story implementation can now begin in parallel

---

## Phase 3: User Story 1 - 查看游戏别名列表 (Priority: P1) 🎯 MVP

**Goal**: Display paginated list of games with aliases, support search by name or alias, show action buttons based on role

**Independent Test**: Can independently test list loading, pagination, search accuracy, and role-based button visibility

### Tests for User Story 1 ⚠️

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T009a [P] [US1] Contract test for `GET /api/game-aliases` endpoint in `crates/hiveweb/src/api/game_test.rs` — verify 200 response with pagination data, search by name returns matching games, search by alias returns matching games, empty search returns all games, invalid page/params return validation error
- [x] T009b [P] [US1] Contract test for `GET /api/game-aliases/:id` endpoint in `crates/hiveweb/src/api/game_test.rs` — verify 200 with game details, 4001 for non-existent id
- [x] T009c [US1] Unit tests for `list_games()` service function in `crates/hiveweb/src/services/game_service_test.rs` — verify search logic (name LIKE + EXISTS alias LIKE), pagination (LIMIT/OFFSET), empty result handling, JSON_ARRAYAGG null handling
- [x] T009d [US1] Unit tests for `get_game_by_id()` service function in `crates/hiveweb/src/services/game_service_test.rs` — verify returns Game with aliases, returns None for non-existent id
- [x] T009e [P] [US1] Component test for `GameTable` in `web-admin/src/components/__tests__/GameAliasComponents.test.tsx` — verify column rendering (ID, name, aliases as Tags, dates), pagination controls, search form, role-based button visibility (System/Super show Edit/Delete, Normal hidden)

### Implementation for User Story 1

- [x] T010 [US1] Implement `list_games()` service function in `crates/hiveweb/src/services/game_service.rs` — accepts (pool, page, page_size, q) params, executes LEFT JOIN + JSON_ARRAYAGG query with search (name LIKE %q% OR EXISTS alias LIKE %q%), returns GameListResponse; implement `get_game_by_id()` for detail view
- [x] T011 [US1] Implement list and detail API handlers in `crates/hiveweb/src/api/game.rs` — `GET /api/game-aliases` handler (pagination + search query params, calls list_games), `GET /api/game-aliases/:id` handler (calls get_game_by_id, returns 4001 if not found), register in `pub fn router() -> Router<AppState>`
- [x] T012 [US1] Create frontend API service in `web-admin/src/services/gameAlias.ts` — TypeScript interfaces (Game, GameListResponse, GameListParams), `getGames(params)` and `getGameById(id)` functions calling `/api/game-aliases`
- [x] T013 [US1] Create `useGameAlias` custom hook in `web-admin/src/hooks/useGameAlias.ts` — manages state (games, loading, pagination, search text), exposes `fetchGames`, `handleSearch`, `handlePageChange`, uses `getGames` from service
- [x] T014 [US1] Create `GameTable` component in `web-admin/src/components/GameTable.tsx` — Ant Design Table with columns (ID, name, aliases as Tags, created_at, updated_at, actions), search form (Input.Search for keyword), pagination, action column with Edit/Delete buttons conditionally rendered by role (System/Super show, Normal hidden)
- [x] T015 [US1] Create `GameAliasPage` page in `web-admin/src/pages/GameAliasPage.tsx` — wires useGameAlias hook to GameTable, shows "添加游戏别名" button (hidden for Normal role), initial data fetch on mount
- [x] T016 [US1] Add `/game-aliases` route in `web-admin/src/App.tsx` (Route path="game-aliases" element=GameAliasPage under Layout) and add menu item in `web-admin/src/components/Layout.tsx` (under 系统管理 group, accessible to all roles)

**Checkpoint**: At this point, User Story 1 should be fully functional — admin can view list, search, paginate, see role-based buttons

---

## Phase 4: User Story 2 - 添加游戏别名 (Priority: P2)

**Goal**: System/Super admins can create new game records with name and aliases using Tags input component

**Independent Test**: Can independently test form submission, validation, data persistence, and list refresh

### Tests for User Story 2 ⚠️

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T016a [P] [US2] Contract test for `POST /api/game-aliases` endpoint in `crates/hiveweb/src/api/game_test.rs` — verify 201 on success with valid body, 403 for Normal role (error code 2001), 400 for empty name (4003), 400 for name too long (4004), 400 for empty aliases (4005), 400 for alias too long (4006), 400 for too many aliases (4007), 409 for duplicate name (4002), 409 for duplicate alias (4008)
- [x] T016b [US2] Unit tests for `create_game()` service function in `crates/hiveweb/src/services/game_service_test.rs` — verify validation (empty name, long name, empty aliases, >20 aliases, long alias), HashSet deduplication of aliases within same request, transactional INSERT (games → alias rows), UNIQUE conflict handling (GAME_NAME_ALREADY_EXISTS / ALIAS_ALREADY_IN_USE), audit log write via `write_audit_log(pool, operator_id, game_id, Operation::GAME_ALIAS_CREATE)`
- [x] T016c [P] [US2] Component test for `GameAliasForm` in create mode in `web-admin/src/components/__tests__/GameAliasComponents.test.tsx` — verify form renders with name Input and aliases Select tags, validation rules (required name, required aliases, max lengths), Submit button disabled until valid, front-end alias deduplication via Set before submit

### Implementation for User Story 2

- [x] T017 [US2] Implement `create_game()` service function in `crates/hiveweb/src/services/game_service.rs` — accepts (pool, name, aliases), validates inputs (name non-empty/≤50, aliases non-empty/≤20 items/each≤50, deduplicate aliases via HashSet), executes transactional INSERT (games → alias rows), handles MySQL UNIQUE constraint violation → return 4008 ALIAS_ALREADY_IN_USE, writes audit log via `write_audit_log(pool, operator_id, game_id, Operation::GAME_ALIAS_CREATE)`
- [x] T018 [US2] Implement `POST /api/game-aliases` API handler in `crates/hiveweb/src/api/game.rs` — extracts JSON body (CreateGameRequest), checks caller role (System/Super only, else 403 + 2001), calls create_game(), returns GameResponse with 201 status
- [x] T019 [US2] Add `createGame(data)` function in `web-admin/src/services/gameAlias.ts` — POST to `/api/game-aliases` with CreateGameRequest body
- [x] T020 [US2] Create `GameAliasForm` component in `web-admin/src/components/GameAliasForm.tsx` — Ant Design Modal + Form, fields: name (Input, required, max 50), aliases (Select mode="tags", required, max 20 items, each max 50 chars), front-end deduplication via Set before submit, validation rules, create/edit mode via `visible` + `editingGame` props
- [x] T021 [US2] Integrate GameAliasForm into GameAliasPage — wire "添加游戏别名" button to open form in create mode, on successful submit call fetchGames to refresh list, show message.success

**Checkpoint**: At this point, User Stories 1 AND 2 should both work independently — admin can view list and add new games

---

## Phase 5: User Story 3 - 编辑游戏别名 (Priority: P2)

**Goal**: System/Super admins can edit existing game records, modifying name or alias list, with updated_at auto-refresh

**Independent Test**: Can independently test form editing, data update, timestamp change, and validation

### Tests for User Story 3 ⚠️

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T021a [P] [US3] Contract test for `PUT /api/game-aliases/:id` endpoint in `crates/hiveweb/src/api/game_test.rs` — verify 200 on success with valid body, 403 for Normal role (error code 2001), 4001 for non-existent id, 400 for validation errors (same rules as create), 409 for duplicate name (4002), 409 for duplicate alias (4008)
- [x] T021b [US3] Unit tests for `update_game()` service function in `crates/hiveweb/src/services/game_service_test.rs` — verify partial update (name only, aliases only, both), validation rules (same as create but fields optional, at least one required), transactional UPDATE (games SET name?, updated_at → DELETE old aliases → INSERT new aliases), UNIQUE conflict handling, audit log write via `write_audit_log(pool, operator_id, id, Operation::GAME_ALIAS_UPDATE)`
- [x] T021c [P] [US3] Component test for `GameAliasForm` in edit mode in `web-admin/src/components/__tests__/GameAliasComponents.test.tsx` — verify form pre-fills with existing name and aliases as tags, submit calls updateGame not createGame, validation same as create mode

### Implementation for User Story 3

- [x] T022 [US3] Implement `update_game()` service function in `crates/hiveweb/src/services/game_service.rs` — accepts (pool, id, name?, aliases?), validates inputs (same rules as create, but fields optional, at least one required), executes transactional UPDATE (games SET name?, updated_at → DELETE old aliases → INSERT new aliases), handles MySQL UNIQUE constraint violation → return 4002 or 4008, writes audit log via `write_audit_log(pool, operator_id, id, Operation::GAME_ALIAS_UPDATE)`
- [x] T023 [US3] Implement `PUT /api/game-aliases/:id` API handler in `crates/hiveweb/src/api/game.rs` — extracts path id + JSON body (UpdateGameRequest), checks role (System/Super only), calls update_game(), returns 4001 if not found
- [x] T024 [US3] Add `updateGame(id, data)` function in `web-admin/src/services/gameAlias.ts` — PUT to `/api/game-aliases/:id` with UpdateGameRequest body
- [x] T025 [US3] Wire GameAliasForm for edit mode — on Edit button click in GameTable, open form with pre-filled data (name + aliases as tags), on submit call updateGame, refresh list

**Checkpoint**: At this point, User Stories 1, 2, AND 3 should all work independently

---

## Phase 6: User Story 4 - 删除游戏别名 (Priority: P3)

**Goal**: System/Super admins can delete game records with confirmation dialog; deletes cascade to alias entries

**Independent Test**: Can independently test deletion flow, confirmation dialog, data removal, and role restriction

### Tests for User Story 4 ⚠️

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T025a [P] [US4] Contract test for `DELETE /api/game-aliases/:id` endpoint in `crates/hiveweb/src/api/game_test.rs` — verify 200 on success, 403 for Normal role (error code 2001), 4001 for non-existent id, verify CASCADE deletes alias entries (query game_alias_entries after delete → empty)
- [x] T025b [US4] Unit tests for `delete_game()` service function in `crates/hiveweb/src/services/game_service_test.rs` — verify deletes from games table, verify CASCADE deletes alias rows (FK ON DELETE CASCADE), returns None/error for non-existent id, audit log write via `write_audit_log(pool, operator_id, id, Operation::GAME_ALIAS_DELETE)`
- [x] T025c [P] [US4] Component test for delete confirmation in `web-admin/src/components/__tests__/GameAliasComponents.test.tsx` — verify Popconfirm appears on Delete click, Cancel does nothing, Confirm calls deleteGame and shows success message

### Implementation for User Story 4

- [x] T026 [US4] Implement `delete_game()` service function in `crates/hiveweb/src/services/game_service.rs` — accepts (pool, id), deletes from games table (CASCADE deletes alias rows automatically via FK ON DELETE CASCADE), writes audit log via `write_audit_log(pool, operator_id, id, Operation::GAME_ALIAS_DELETE)`
- [x] T027 [US4] Implement `DELETE /api/game-aliases/:id` API handler in `crates/hiveweb/src/api/game.rs` — extracts path id, checks role (System/Super only), calls delete_game(), returns 4001 if not found
- [x] T028 [US4] Add `deleteGame(id)` function in `web-admin/src/services/gameAlias.ts` — DELETE to `/api/game-aliases/:id`
- [x] T029 [US4] Add delete confirmation in GameTable — Ant Design Popconfirm on Delete button, on confirm call deleteGame, show message.success, refresh list

**Checkpoint**: All user stories should now be independently functional

---

## Phase N: Polish & Cross-Cutting Concerns

**Purpose**: Improvements that affect multiple user stories

- [x] T030 [P] Run `cargo fmt` and `cargo clippy -- -D warnings` on hiveweb crate, fix all warnings
- [x] T031 [P] Run `npm run lint` on web-admin — 0 new ESLint warnings from game alias code; all 39 pre-existing errors/warnings are from other pages
- [x] T032 Verify migration application by running `cargo run --bin migrate` and confirming both V045 and V046 are applied successfully
- [ ] T033 Manual end-to-end test: login as Normal admin → view list only; login as System admin → full CRUD
- [ ] T034 Verify alias global uniqueness: create Game A with alias "X", attempt to create Game B with alias "X" → should get 4008 error
- [ ] T035 Verify a11y: run axe-core on GameAliasPage → 0 critical/serious violations
- [x] T036 Run quickstart.md validation — all curl commands use correct routes (`/api/game-aliases`), correct HTTP methods (GET/POST/PUT/DELETE), correct Content-Type headers; migration file names corrected from V007/V008 to V045/V046
- [x] T037 [P] [US4] Performance verification: Insert 1000 game records with 20 aliases each via seed script, measure GET /api/game-aliases response time (p95, p99) using `curl -w '%{time_total}'` or `hyperfine` — MUST meet < 200ms p95 budget; document results in plan.md Complexity Tracking section
- [x] T038 [P] [US4] Verify SC-001: Load GameAliasPage in browser with Network throttling set to Fast 3G, measure Time to Interactive — MUST be < 2 seconds for first 10 rows; run `EXPLAIN` on list query to verify index usage (idx_game_name, idx_alias)
- [x] T039 Audit UI completeness review: verified all role-gated UI elements follow "无权限 = 不渲染" convention — GameAliasPage (add button), GameTable (Edit/Delete), Layout.tsx (sidebar menu) all conditionally rendered; no leaked controls accessible to Normal role

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies - can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion - BLOCKS all user stories
- **User Stories (Phase 3-6)**: All depend on Foundational phase completion
  - User stories can then proceed in parallel (if staffed)
  - Or sequentially in priority order (P1 → P2 → P2 → P3)
- **Polish (Phase N)**: Depends on all desired user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational (Phase 2) — No dependencies on other stories
- **User Story 2 (P2)**: Can start after Foundational (Phase 2) — Reuses GameTable + GameAliasForm from US1
- **User Story 3 (P2)**: Can start after Foundational (Phase 2) — Reuses GameAliasForm from US2 in edit mode
- **User Story 4 (P3)**: Can start after Foundational (Phase 2) — Reuses GameTable from US1

### Within Each User Story

- Tests MUST be written and verified to FAIL before implementation (Constitution Principle II)
- Models before services
- Services before API handlers
- Backend before frontend service
- Frontend service before components
- Core implementation before integration
- Story complete before moving to next priority

### Parallel Opportunities

- T001/T002/T003/T004/T005 (Setup) can all run in parallel
- T006/T007 (error codes + audit types) can run in parallel
- T008/T009 (model + DTOs) are sequential
- Within each user story: all test tasks marked [P] can run in parallel
- Once Foundational (T006-T009) completes, all user stories can start in parallel
- T017/T019 (backend create + frontend service) can run in parallel across different files
- Different user stories can be worked on in parallel by different team members

---

## MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001-T005)
2. Complete Phase 2: Foundational (T006-T009) — **CRITICAL - blocks all stories**
3. Complete Phase 3 Tests: (T009a-T009e) — write tests first, verify they fail
4. Complete Phase 3 Implementation: (T010-T016)
5. **STOP and VALIDATE**: Admin can log in, view game alias list with pagination and search, see role-appropriate buttons
6. Deploy/demo if ready

## Incremental Delivery

1. Complete Setup + Foundational → Foundation ready
2. Add User Story 1 (tests + list + search + pagination) → Test independently → Deploy/Demo (MVP!)
3. Add User Story 2 (tests + create) → Test independently → Deploy/Demo
4. Add User Story 3 (tests + edit) → Test independently → Deploy/Demo
5. Add User Story 4 (tests + delete) → Test independently → Deploy/Demo
6. Each story adds value without breaking previous stories

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- **Test-first is NON-NEGOTIABLE per Constitution Principle II**: write tests, verify failure, then implement
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- Migration versions: V045 (games), V046 (game_alias_entries) — latest existing is V044
- Alias editor: Ant Design `Select mode="tags"` — user types alias, presses Enter to add as tag, clicks X to remove
- Alias deduplication: Frontend uses Set to deduplicate before submit; backend uses HashSet + DB UNIQUE index as final authority
- Alias global uniqueness: DB UNIQUE constraint on `game_alias_entries.alias` — insert conflict returns 4008 ALIAS_ALREADY_IN_USE
- Transaction pattern: Create = INSERT game → INSERT N alias rows; Update = UPDATE game → DELETE old aliases → INSERT new aliases
- Audit logging: Every create/update/delete service function MUST call `write_audit_log()` with operator_id, target_game_id, operation type, timestamp
- Performance budget: API p95 < 200ms (Constitution Principle IV), list load < 2s first screen (SC-001)
