# Tasks: 敏感词过滤

**Input**: Design documents from `/specs/010-sensitive-word-filter/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/api.md

**Tests**: Tests are included per Constitution Principle II (Test-First Development).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

- **Backend**: `crates/hiveweb/src/`
- **Frontend**: `web-admin/src/`
- **Tests**: `crates/hiveweb/tests/`
- **Migrations**: `crates/hiveweb/src/db/`

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Database schema and shared types needed by all user stories

- [x] T001 Create DB migration file `crates/hiveweb/migrations/V025__create_sensitive_words.sql` with `sensitive_words` and `sensitive_filter_logs` tables per data-model.md
- [x] T001b [P] Add `aho-corasick` dependency to `crates/hiveweb/Cargo.toml` (workspace dependency)
- [x] T002 [P] Create model struct `SensitiveWord` with sqlx::FromRow in `crates/hiveweb/src/models/sensitive_word.rs`
- [x] T003 [P] Create request/response types (`CreateSensitiveWordRequest`, `UpdateSensitiveWordRequest`, `SensitiveWordResponse`, `SensitiveWordListResponse`) in `crates/hiveweb/src/models/sensitive_word.rs`
- [x] T004 Register `sensitive_word` module in `crates/hiveweb/src/models/mod.rs` and `crates/hiveweb/src/services/mod.rs` and `crates/hiveweb/src/api/mod.rs`
- [x] T004b Create seed script `crates/hiveweb/src/bin/seed_sensitive_words.rs` that reads pre-merged JSON word list and INSERTs entries with duplicate skip

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core filter engine — shared by US1 (input) and US2 (output). MUST complete before any user story.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

### Tests for Foundational

- [x] T005 [P] Write unit test for `SensitiveFilter::check()` — exact match hits and misses in `crates/hiveweb/tests/it_sensitive_filter.rs`
- [x] T006 [P] Write unit test for `SensitiveFilter::check()` — regex match hits and misses in `crates/hiveweb/tests/it_sensitive_filter.rs`
- [x] T007 [P] Write unit test for cache refresh — filter picks up newly added word after reload in `crates/hiveweb/tests/it_sensitive_filter.rs`

### Implementation for Foundational

- [x] T009 Implement `SensitivePattern` enum (Exact/Regex variants) + `SensitiveFilter` struct with `Arc<RwLock<Vec<SensitivePattern>>>` in `crates/hiveweb/src/services/sensitive_filter.rs`
- [x] T010 Implement `SensitiveFilter::load_from_db()` — load all enabled words from `sensitive_words` table and compile patterns in `crates/hiveweb/src/services/sensitive_filter.rs`
- [x] T011 Implement `SensitiveFilter::check(text: &str) -> Option<&SensitivePattern>` — Aho-Corasick for exact + cached regex iteration in `crates/hiveweb/src/services/sensitive_filter.rs`
- [x] T012 Implement `SensitiveFilter::refresh_cache()` — reload from DB and replace in-memory cache in `crates/hiveweb/src/services/sensitive_filter.rs`
- [x] T013 Implement `log_filter_event()` — write to `sensitive_filter_logs` table (fire-and-forget) in `crates/hiveweb/src/services/sensitive_filter.rs`
- [x] T014 Initialize `SensitiveFilter` and inject into `AppState` at server startup in `crates/hiveweb/src/api/mod.rs`

**Checkpoint**: Filter engine ready — can load words from DB, check text, and refresh cache.

---

## Phase 3: User Story 1 — 输入包含敏感词时拦截 (Priority: P1) 🎯 MVP

**Goal**: 用户发送包含敏感词的消息时，`/api/assistant` 立即拒绝并返回 `{"reply": "...", "filtered": true}`

**Independent Test**: 发 POST 到 `/api/assistant`，message 包含已知敏感词，验证返回 `filtered: true` 且不进入 Agent 流程

### Tests for User Story 1

- [x] T015 [P] [US1] Contract test: POST `/api/assistant` with message containing known exact-match sensitive word → HTTP 200, `{"reply": "...", "filtered": true}` in `crates/hiveweb/tests/it_sensitive_filter.rs`
- [x] T016 [P] [US1] Contract test: POST `/api/assistant` with clean message → normal Agent response (no filtered flag) in `crates/hiveweb/tests/it_sensitive_filter.rs`
- [x] T017 [US1] Contract test: POST `/api/assistant` with empty sensitive words table → all messages pass in `crates/hiveweb/tests/it_sensitive_filter.rs`

### Implementation for User Story 1

- [x] T018 [US1] Call `SensitiveFilter::check()` on `req.message` in `assistant_chat()` handler — after message non-empty validation, before external DB checks, in `crates/hiveweb/src/api/chat_assistant.rs`

**Checkpoint**: US1 complete — input filtering works end-to-end for the assistant API.

---

## Phase 4: User Story 2 — Agent 输出包含敏感词时替换 (Priority: P2)

**Goal**: Agent 生成的回复包含敏感词时，将输出替换为友好提示

**Independent Test**: 模拟 Agent 输出包含敏感词，验证最终返回 `filtered: true` 且原始内容被替换

### Tests for User Story 2

- [x] T020 [P] [US2] Integration test: Agent output contains exact-match sensitive word → response replaced with friendly prompt in `crates/hiveweb/tests/it_sensitive_filter.rs`
- [x] T021 [P] [US2] Integration test: Agent output contains regex-match sensitive word → response replaced in `crates/hiveweb/tests/it_sensitive_filter.rs`
- [x] T022 [US2] Integration test: Agent output is clean → response returned unmodified in `crates/hiveweb/tests/it_sensitive_filter.rs`

### Implementation for User Story 2

- [x] T023 [US2] Add output filter check in `run_session_internal_impl()` — after `final_content` is set and before `finalize_with_variant()`, check content against `SensitiveFilter` in `crates/hiveweb/src/runtime/orchestrator.rs`
- [x] T024 [US2] When output matches, replace `final_content` with friendly prompt and set `filtered` flag in response in `crates/hiveweb/src/runtime/orchestrator.rs`

**Checkpoint**: US2 complete — both input and output filtering work.

---

## Phase 5: User Story 3 — 管理员管理敏感词库 (Priority: P3)

**Goal**: 管理员通过后台 CRUD 敏感词，变更即时生效

**Independent Test**: 管理员添加一条敏感词 → 立即通过 `/api/assistant` 验证拦截

### Tests for User Story 3

- [x] T025 [P] [US3] Contract test: POST `/api/sensitive-words` creates a new word and returns 200 in `crates/hiveweb/tests/it_sensitive_filter.rs`
- [x] T026 [P] [US3] Contract test: POST with invalid regex returns 400 in `crates/hiveweb/tests/it_sensitive_filter.rs`
- [x] T027 [P] [US3] Contract test: POST duplicate word returns 409 in `crates/hiveweb/tests/it_sensitive_filter.rs`
- [x] T028 [P] [US3] Contract test: PUT updates word and triggers cache refresh in `crates/hiveweb/tests/it_sensitive_filter.rs`
- [x] T029 [P] [US3] Contract test: DELETE removes word and triggers cache refresh in `crates/hiveweb/tests/it_sensitive_filter.rs`
- [x] T030 [P] [US3] Contract test: GET list with search and pagination in `crates/hiveweb/tests/it_sensitive_filter.rs`
- [x] T030b [US3] Unit test: `regex::Regex::new()` compilation failure correctly rejected at create time in `crates/hiveweb/tests/it_sensitive_filter.rs`

### Implementation for User Story 3

- [x] T031 [P] [US3] Implement `list_sensitive_words()` query with search + pagination in `crates/hiveweb/src/services/sensitive_filter.rs`
- [x] T032 [P] [US3] Implement `create_sensitive_word()` with validation (non-empty, ≤512 chars, regex validity check, duplicate check) + auto-refresh cache in `crates/hiveweb/src/services/sensitive_filter.rs`
- [x] T033 [P] [US3] Implement `update_sensitive_word()` with same validation + auto-refresh cache in `crates/hiveweb/src/services/sensitive_filter.rs`
- [x] T034 [P] [US3] Implement `delete_sensitive_word()` + auto-refresh cache in `crates/hiveweb/src/services/sensitive_filter.rs`
- [x] T035 [US3] Create API handler `crates/hiveweb/src/api/sensitive_word.rs` with routes: GET `/sensitive-words`, POST `/sensitive-words`, PUT `/sensitive-words/:id`, DELETE `/sensitive-words/:id`, all protected by admin auth
- [x] T036 [P] [US3] Create admin page `web-admin/src/pages/SensitiveWordPage.tsx` with table list, search, add/edit/delete dialogs
- [x] T037 [P] [US3] Create API client `web-admin/src/services/sensitiveWord.ts` with typed request/response
- [x] T038 [US3] Add route and navigation entry for SensitiveWordPage in `web-admin/src/App.tsx` and `web-admin/src/components/Layout.tsx`

**Checkpoint**: US3 complete — full admin CRUD with instant cache refresh.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Refinements that span multiple user stories

- [x] T039 [P] Add structured logging (tracing::info) for filter events with request_id correlation in `crates/hiveweb/src/services/sensitive_filter.rs`
- [x] T040 [P] Run `cargo fmt` and `cargo clippy -- -D warnings` on all new backend code
- [x] T041 [P] Run `cargo test -p hiveweb --test it_sensitive_filter` — all tests must pass
- [x] T042 Verify quickstart.md steps work end-to-end
- [x] T043 Add performance benchmark: `SensitiveFilter::check()` with 10,000 words completes in < 1ms in `crates/hiveweb/tests/it_sensitive_filter.rs`

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Setup — **BLOCKS all user stories**
- **US1 (Phase 3)**: Depends on Foundational
- **US2 (Phase 4)**: Depends on Foundational (SensitiveFilter is in AppState from T014)
- **US3 (Phase 5)**: Depends on Foundational (can run in parallel with US1/US2)
- **Polish (Phase 6)**: Depends on all user stories complete

### User Story Dependencies

```
Phase 2 (Foundational)
    │
    ├──> Phase 3: US1 (P1) ← Input filter
    │
    ├──> Phase 4: US2 (P2) ← Output filter (independent of US1)
    │
    └──> Phase 5: US3 (P3) ← Admin CRUD
```

### Within Each Phase

- Tests MUST be written first and fail before implementation (TDD per Constitution)
- Models before services, services before endpoints
- Backend endpoints before frontend UI

### Parallel Opportunities

- Phase 1: T002, T003 can run in parallel
- Phase 2: T005, T006, T007 can run in parallel
- Phase 3: T015, T016 can run in parallel
- Phase 4: T020, T021 can run in parallel
- Phase 5: T025-T030 (all tests) can run in parallel; T031-T034 (all services) can run in parallel; T036, T037 can run in parallel
- Phase 6: T039, T040, T041 can run in parallel

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL)
3. Complete Phase 3: User Story 1
4. **STOP and VALIDATE**: Run `cargo test -p hiveweb --test it_sensitive_filter` — verify input filtering works
5. Deploy — users are protected from sending sensitive content

### Incremental Delivery

1. Setup + Foundational → Filter engine ready
2. US1 → Input filtering live (MVP!)
3. US2 → Output filtering live (can run in parallel with US1 if team capacity allows)
4. US3 → Admin can manage sensitive words without dev support
5. Polish → Production ready
