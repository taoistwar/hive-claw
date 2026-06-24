# Tasks: Agent Context 能力设计

**Input**: Design documents from `/specs/009-agent-context/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/api.md, quickstart.md

**Tests**: Tests are required per Constitution Principle II (Test-First Development). Each phase includes test tasks before implementation.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

- **Project**: `crates/agent/` — Rust library crate
- Source: `crates/agent/src/`
- Tests: `crates/agent/tests/`

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Create Agent Context module structure and dependencies

- [x] T001 Create `crates/agent/src/context/` directory structure
- [x] T002 [P] Add `serde` / `serde_json` dependency verification in `crates/agent/Cargo.toml`
- [x] T003 [P] Create `crates/agent/src/context/mod.rs` with module declarations and re-exports
- [x] T004 [P] Create empty source files: `core.rs`, `category.rs`, `lock.rs`, `read_view.rs`, `merge.rs`, `prompt.rs`, `response.rs`, `serialize.rs`, `audit.rs`, `config.rs` in `crates/agent/src/context/`
- [x] T005 Create `crates/agent/tests/` directory and add empty test files: `context_basic.rs`, `context_concurrent.rs`, `context_merge.rs`, `context_serialize.rs`, `context_prompt.rs`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core data structures, concurrency model, and lifecycle management that ALL user stories depend on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

### Tests for Foundational Layer

- [x] T006 [P] Write unit test for `Category` enum and `RecordEntry` struct in `crates/agent/tests/context_basic.rs`
- [x] T007 [P] Write unit test for `ContextConfig` with default and custom thresholds in `crates/agent/tests/context_basic.rs`
- [x] T008 [P] Write unit test for `LifecycleState` transitions in `crates/agent/tests/context_basic.rs`
- [x] T009 [P] Write unit test for `ContextError` variants in `crates/agent/tests/context_basic.rs`

### Implementation for Foundational Layer

- [x] T010 Define `Category` enum, `RecordEntry` struct, `ToolCallStatus` enum in `crates/agent/src/context/category.rs`
- [x] T011 Define `LifecycleState` enum with valid transition rules in `crates/agent/src/context/core.rs`
- [x] T012 Define `ContextConfig` struct with soft_limit defaults and token estimation config in `crates/agent/src/context/config.rs`
- [x] T013 Define `ContextError` enum with all error variants in `crates/agent/src/context/core.rs`
- [x] T014 Implement `CategoryLock<T>` wrapper around `Arc<RwLock<T>>` with soft limit checking and warn! logging in `crates/agent/src/context/lock.rs`
- [x] T015 Implement `UserInput` struct with serde Serialize/Deserialize in `crates/agent/src/context/core.rs`
- [x] T016 Update `crates/agent/src/lib.rs` with `pub mod context;` and re-exports of `AgentContext`, `ReadView`, `Category`, `ContextError`, `ContextConfig`

**Checkpoint**: Foundation ready — data structures, concurrency primitives, and config are in place.

---

## Phase 3: User Story 1 — Agent 执行过程中统一状态管理 (Priority: P1) 🎯 MVP

**Goal**: Agent Context 核心读写 API，支持用户输入保存、分类数据存储、按类别查询、并发安全读取

**Independent Test**: 可以独立构造一个 Agent Context 实例，写入用户输入和若干中间状态（如实体、Tool 结果），然后从 Context 中正确读取这些值。

### Tests for User Story 1

- [x] T017 [P] [US1] Test: create AgentContext, verify user_input is accessible in `crates/agent/tests/context_basic.rs`
- [x] T018 [P] [US1] Test: set_record + get_record roundtrip for a single key in `crates/agent/tests/context_basic.rs`
- [x] T019 [P] [US1] Test: get_category returns all records for a category in `crates/agent/tests/context_basic.rs`
- [x] T020 [P] [US1] Test: concurrent read access from 10 threads does not deadlock or panic in `crates/agent/tests/context_concurrent.rs`
- [x] T021 [US1] Test: lifecycle state transitions (Active → Completed, Active → Terminated) are valid; illegal transitions return ContextError in `crates/agent/tests/context_basic.rs`
- [x] T102 [P] [US1] Test: ReadView provides read-only access and does not allow mutation (FR-013) in `crates/agent/tests/context_basic.rs`

### Implementation for User Story 1

- [x] T022 [P] [US1] Implement `AgentContext::new()` constructor with user_input, config, category initialization in `crates/agent/src/context/core.rs`
- [x] T023 [US1] Implement `AgentContext::user_input()` read accessor in `crates/agent/src/context/core.rs`
- [x] T024 [US1] Implement `AgentContext::set_record()` with category lock acquisition, soft limit check, StateChangeLog recording in `crates/agent/src/context/core.rs`
- [x] T025 [US1] Implement `AgentContext::get_record()` read accessor in `crates/agent/src/context/core.rs`
- [x] T026 [US1] Implement `AgentContext::get_category()` returning all records for a category in `crates/agent/src/context/core.rs`
- [x] T027 [US1] Implement `AgentContext::append_record()` for non-overlapping writes in `crates/agent/src/context/core.rs`
- [x] T028 [US1] Implement `AgentContext::lifecycle_state()` and `AgentContext::set_lifecycle_state()` with transition validation in `crates/agent/src/context/core.rs`
- [x] T029 [P] [US1] Implement `ReadView` struct with read-only accessors in `crates/agent/src/context/read_view.rs`
- [x] T030 [US1] Implement `AgentContext::read_view()` method returning `ReadView` in `crates/agent/src/context/core.rs`
- [x] T031 [US1] Implement `AgentContext::add_extension()` for storing ExtensionContent in `crates/agent/src/context/core.rs`
- [x] T032 [US1] Implement `AgentContext::get_extensions()` in `crates/agent/src/context/core.rs`
- [x] T033 [US1] Implement `StateChangeLog` recording in `AgentContext::set_record()` path in `crates/agent/src/context/audit.rs`

**Checkpoint**: User Story 1 is fully functional — Context can be created, written to, read from, and queried by category with thread safety.

---

## Phase 4: User Story 2 — Tool 和 Skill 通过 Context 写入执行状态 (Priority: P2)

**Goal**: Tool/Skill 可通过受控 API 写入状态；执行轨迹记录（ToolCallRecord、SkillExecutionRecord）完整可用

**Independent Test**: 构造 Context 和模拟 Tool，Tool 执行后写入 Context 特定分类，验证后续读取正确。

### Tests for User Story 2

- [x] T034 [P] [US2] Test: Tool writes result to ToolResults category, subsequent read returns correct data in `crates/agent/tests/context_basic.rs`
- [x] T035 [P] [US2] Test: soft limit exceeded triggers warn! log but does not reject write in `crates/agent/tests/context_basic.rs`
- [x] T036 [US2] Test: StateChangeLog contains old_value and new_value after overwrite in `crates/agent/tests/context_basic.rs`

### Implementation for User Story 2

- [x] T037 [P] [US2] Implement `ToolCallRecord` struct with serde in `crates/agent/src/context/audit.rs`
- [x] T038 [P] [US2] Implement `SkillExecutionRecord` struct with serde in `crates/agent/src/context/audit.rs`
- [x] T039 [US2] Implement `AuditLogger` internal helper struct with `record_tool_call()`, `record_skill_execution()` methods in `crates/agent/src/context/audit.rs`
- [x] T040 [US2] Integrate AuditLogger into `AgentContext` — `AgentContext::record_tool_call()` and `AgentContext::record_skill_execution()` delegate to AuditLogger internally in `crates/agent/src/context/core.rs`
- [x] T041 [US2] Implement `AgentContext::get_audit_log()` accessor in `crates/agent/src/context/audit.rs`
- [x] T042 [P] [US2] Implement `AgentContextSyncHook` struct in new file `crates/agent/src/context/hook.rs` (extending existing hook.rs is not recommended to avoid circular dependencies)
- [x] T043 [US2] Implement `AgentContextSyncHook::after_tool_execution()` to sync Tool results to Context in `crates/agent/src/context/hook.rs`
- [x] T044 [US2] Implement `AgentContextSyncHook::after_iteration()` to sync iteration state to Context in `crates/agent/src/context/hook.rs`

**Checkpoint**: User Story 2 is fully functional — Tool/Skill can write to Context, audit log tracks all operations.

---

## Phase 5: User Story 3 — 子 Agent 协作中的 Context 读取与合并 (Priority: P3)

**Goal**: 子 Agent 可 fork Context（黑名单模式），执行后合并回主 Context（主优先 + 独立命名空间）

**Independent Test**: 构造主 Context 和子 Agent Context，传递部分数据给子 Context，子 Context 执行后合并结果回主 Context，验证数据完整性。

### Tests for User Story 3

- [x] T045 [P] [US3] Test: fork_for_subagent creates child with all categories accessible (no blacklist) in `crates/agent/tests/context_merge.rs`
- [x] T046 [P] [US3] Test: fork_for_subagent with blacklist — child cannot write to blacklisted categories, returns ContextError::BlacklistedCategoryWrite in `crates/agent/tests/context_merge.rs`
- [x] T047 [US3] Test: merge_subagent_context — child's new data stored under `{subagent_id}/` prefix, parent's existing data not overwritten in `crates/agent/tests/context_merge.rs`
- [x] T048 [US3] Test: merge with conflict — child's conflicting data stored under `{subagent_id}/conflict/` in `crates/agent/tests/context_merge.rs`
- [x] T098 [P] [US3] Test: fork_for_subagent with UserInput in blacklist returns InvalidBlacklist error (FR-006) in `crates/agent/tests/context_merge.rs`
- [x] T099 [US3] Test: nested subagent fork→fork→merge — delegation chain prefix extends correctly (FR-018, e.g., sub-001/sub-002/{category}/) in `crates/agent/tests/context_merge.rs`

### Implementation for User Story 3

- [x] T049 [P] [US3] Implement `AgentDelegationRecord` struct in `crates/agent/src/context/audit.rs`
- [x] T050 [US3] Implement `AgentContext::fork_for_subagent()` with blacklist validation, shallow copy of category data in `crates/agent/src/context/merge.rs`
- [x] T051 [US3] Implement blacklist write guard — child Context rejects writes to blacklisted categories with `ContextError::BlacklistedCategoryWrite` in `crates/agent/src/context/merge.rs`
- [x] T052 [US3] Implement `AgentContext::merge_subagent_context()` — iterate child categories, apply main-priority rule, namespace child data with full delegation chain prefix (e.g., `sub-001/sub-002/{category}/` for nested subagents) in `crates/agent/src/context/merge.rs`
- [x] T053 [US3] Implement conflict detection and storage under `{subagent_id}/{category}/conflict/` during merge in `crates/agent/src/context/merge.rs`
- [x] T054 [US3] Record `AgentDelegationRecord` on fork and merge operations in `crates/agent/src/context/audit.rs`
- [x] T055 [US3] Implement `AgentContextSyncHook::before_subagent_fork()` and `after_subagent_merge()` hooks in `crates/agent/src/context/hook.rs`

**Checkpoint**: User Story 3 is fully functional — subagent fork, write restriction, merge with namespace isolation all work.

---

## Phase 6: User Story 4 — 从 Context 构建 LLM Prompt (Priority: P2)

**Goal**: Prompt 构建方法从 Context 提取数据，按固定优先级裁剪 token 超限内容

**Independent Test**: 构造包含多种状态的 Context，调用 Prompt 构建方法，验证生成的 Prompt 包含预期数据片段。

### Tests for User Story 4

- [x] T056 [P] [US4] Test: build_prompt_budget returns user_input always present in `crates/agent/tests/context_prompt.rs`
- [x] T057 [P] [US4] Test: build_prompt_budget with data exceeding max_tokens truncates by priority (history first) in `crates/agent/tests/context_prompt.rs`
- [x] T058 [US4] Test: PromptData.estimated_tokens is accurate, PromptData.truncated is true when truncation occurred in `crates/agent/tests/context_prompt.rs`

### Implementation for User Story 4

- [x] T059 [P] [US4] Implement `PromptData` struct in `crates/agent/src/context/prompt.rs`
- [x] T060 [US4] Implement `AgentContext::build_prompt_budget(max_tokens)` with fixed priority truncation: user_input(100%) > recent_reasoning > tool_results > entities > history in `crates/agent/src/context/prompt.rs`
- [x] T061 [US4] Implement token estimation helper (string length based or integrate tiktoken if available) in `crates/agent/src/context/prompt.rs`
- [x] T062 [US4] Set `PromptData.truncated = true` when any category data is dropped due to budget in `crates/agent/src/context/prompt.rs`
- [x] T063 [US4] Implement `AgentContext::set_response_payload()` for storing final LLM response in `crates/agent/src/context/response.rs`

**Checkpoint**: User Story 4 is fully functional — Prompt building with priority-based truncation works correctly.

---

## Phase 7: User Story 5 — 从 Context 构建最终响应 (Priority: P2)

**Goal**: ResponseBuilder 从 Context 提取文本、扩展内容、推荐问题等构建结构化响应

**Independent Test**: 在 Context 中写入文本、卡片、推荐问题等内容，调用 Response Builder，验证生成的响应包含所有预期字段。

### Tests for User Story 5

- [x] T064 [P] [US5] Test: ResponsePayload contains text and extensions after set_response_payload in `crates/agent/tests/context_basic.rs`
- [x] T065 [P] [US5] Test: ExtensionContent with different types (Card, Suggestion, Link) stored and retrieved correctly in `crates/agent/tests/context_basic.rs`

### Implementation for User Story 5

- [x] T066 [P] [US5] Implement `ExtensionContent` struct with `ExtensionType` enum and `serde` in `crates/agent/src/context/response.rs`
- [x] T067 [P] [US5] Implement `ResponsePayload` struct with text, extensions, object_refs, suggestions in `crates/agent/src/context/response.rs`
- [x] T068 [P] [US5] Implement `ObjectRef` struct in `crates/agent/src/context/response.rs`
- [x] T069 [US5] Implement `AgentContext::set_response_payload()` — store ResponsePayload, update lifecycle to Completed in `crates/agent/src/context/response.rs`
- [x] T070 [US5] Implement `AgentContext::get_response_payload()` accessor in `crates/agent/src/context/response.rs`
- [x] T071 [US5] Implement helper method `build_response_payload()` to extract text + extensions from Context in `crates/agent/src/context/response.rs`

**Checkpoint**: User Story 5 is fully functional — response building from Context works with all content types.

---

## Phase 8: User Story 6 — 执行轨迹与可观测性 (Priority: P3)

**Goal**: 完整执行轨迹记录（Tool 调用、Skill 执行、Agent 委派、状态变更），序列化快照用于调试回放

**Independent Test**: 在 Context 中记录若干执行步骤的轨迹，导出 JSON 轨迹日志，验证包含完整操作序列和时间戳。

### Tests for User Story 6

- [x] T072 [P] [US6] Test: StateChangeLog records all set_record operations with old_value and new_value in `crates/agent/tests/context_basic.rs`
- [x] T073 [P] [US6] Test: ContextSnapshot captures full state at a point in time in `crates/agent/tests/context_serialize.rs`
- [x] T074 [US6] Test: to_json + from_json roundtrip preserves all data (no loss) in `crates/agent/tests/context_serialize.rs`
- [x] T094 [P] [US6] Test: from_json with schema_version mismatch returns SchemaVersionMismatch error (FR-020) in `crates/agent/tests/context_serialize.rs`
- [x] T095 [P] [US6] Test: set_record with sensitive key (api_key, token, password) returns RejectedSensitiveData (FR-021) in `crates/agent/tests/context_basic.rs`
- [x] T096 [P] [US6] Test: set_record with sensitive value object returns RejectedSensitiveData (FR-021) in `crates/agent/tests/context_basic.rs`
- [x] T097 [US6] Test: audit completeness — all tool calls, skill executions, state changes are recorded (SC-006) in `crates/agent/tests/context_basic.rs`

### Implementation for User Story 6

- [x] T075 [P] [US6] Implement `StateChangeLog` struct with serde in `crates/agent/src/context/audit.rs`
- [x] T076 [US6] Implement `ContextSnapshot` struct capturing full state at a point in time in `crates/agent/src/context/serialize.rs`
- [x] T077 [US6] Implement `AgentContext::snapshot()` method creating a deep copy of all state in `crates/agent/src/context/serialize.rs`
- [x] T078 [US6] Implement `AgentContext::to_json()` using serde_json in `crates/agent/src/context/serialize.rs`
- [x] T079 [US6] Implement `AgentContext::from_json()` constructor for deserialization in `crates/agent/src/context/serialize.rs`
- [x] T080 [US6] Implement `AgentContext::get_state_changes()` accessor for StateChangeLog entries in `crates/agent/src/context/audit.rs`

**Checkpoint**: User Story 6 is fully functional — audit trail and serialization for debugging work correctly.

---

## Phase 9: Integration with AgentLoop

**Purpose**: Wire Agent Context into existing `AgentLoop` lifecycle

- [x] T081 Modify `AgentLoop` constructor to accept optional `AgentContext` parameter in `crates/agent/src/loop_.rs`
- [x] T082 Modify `AgentLoop::new()` to create AgentContext from user_input when provided in `crates/agent/src/loop_.rs`
- [x] T083 Register `AgentContextSyncHook` into AgentLoop's hook chain in `crates/agent/src/loop_.rs`
- [x] T084 On AgentLoop completion, update Context lifecycle state to `Completed` in `crates/agent/src/loop_.rs`
- [x] T085 On AgentLoop error/timeout, update Context lifecycle state to `Terminated` and trigger cleanup in `crates/agent/src/loop_.rs`
- [x] T086 Integration test: full AgentLoop run with AgentContext, verify all states recorded correctly in `crates/agent/tests/context_basic.rs`
- [x] T100 [P] Test: AgentContextSyncHook failure logs error but does not interrupt AgentLoop (FR-019) in `crates/agent/tests/context_basic.rs`
- [x] T101 [P] Test: debug_mode=true preserves Context snapshot on termination (IL-004) in `crates/agent/tests/context_serialize.rs`

---

## Phase 10: Polish & Cross-Cutting Concerns

**Purpose**: Improvements that affect multiple user stories

- [x] T087 [P] Add comprehensive doc comments to all public types and methods in `crates/agent/src/context/`
- [x] T088 [P] Add `#[derive(Debug, Clone)]` where appropriate, verify `Send + Sync` bounds for all Context types
- [x] T089 Run `cargo fmt` and `cargo clippy -- -D warnings` on all new code
- [x] T090 [P] Add `Display` / `Debug` implementations for `ContextError`, `LifecycleState`, `Category`
- [x] T091 Run full test suite: `cargo test context` — all tests must pass
- [x] T092 Validate quickstart.md examples compile and run correctly
- [x] T093 Add `#[cfg(test)]` module with inline tests for each context sub-module

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — **BLOCKS all user stories**
- **User Stories (Phase 3-8)**: All depend on Foundational phase completion
  - US1 (P1) → no dependencies on other stories
  - US2 (P2) → depends on US1 foundational API (set_record/get_record)
  - US4 (P2) → depends on US1 (read access to categories)
  - US5 (P2) → depends on US1 (extensions API)
  - US3 (P3) → depends on US1 + US2 (merge requires read + write)
  - US6 (P3) → depends on US1 (audit log infrastructure)
- **Integration (Phase 9)**: Depends on all user stories complete
- **Polish (Phase 10)**: Depends on Integration phase completion

### User Story Dependencies

```
Phase 2 (Foundational)
    │
    ├──> Phase 3: US1 (P1) ← Core Context API
    │         │
    │         ├──> Phase 4: US2 (P2) ← Tool/Skill writes
    │         ├──> Phase 6: US4 (P2) ← Prompt building
    │         └──> Phase 7: US5 (P2) ← Response building
    │                  │
    │                  └──> Phase 5: US3 (P3) ← Subagent merge
    │
    └──> Phase 8: US6 (P3) ← Audit + Serialization (parallel with US2/4/5)
```

### Within Each User Story

- Tests MUST be written first and fail before implementation (TDD per Constitution)
- Models/structs before services/methods
- Core implementation before integration
- Story complete before moving to next priority

### Parallel Opportunities

- Phase 1: T002, T003, T004 can run in parallel
- Phase 2: T006-T009 (tests) can run in parallel; T010, T012, T013 can run in parallel
- Phase 3: T017-T020 (tests) can run in parallel; T022, T029 can run in parallel
- Phase 4: T034, T035 can run in parallel; T037, T038 can run in parallel
- Phase 6: T056, T057 can run in parallel; T059 can run in parallel
- Phase 7: T064, T065 can run in parallel; T066, T067, T068 can run in parallel
- Phase 8: T072, T073 can run in parallel
- Phase 10: T087, T088, T090 can run in parallel

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: User Story 1
4. **STOP and VALIDATE**: Run `cargo test context` — verify all US1 tests pass
5. Context can be created, written to, read from, queried by category

### Incremental Delivery

1. Setup + Foundational → Foundation ready
2. US1 → Core Context API works independently
3. US2 + US4 + US5 (parallel) → Tool writes, Prompt building, Response building
4. US3 → Subagent fork + merge
5. US6 → Audit trail + serialization
6. Phase 9 → AgentLoop integration
7. Phase 10 → Polish

### Parallel Team Strategy

With multiple developers:

1. Team completes Setup + Foundational together
2. Once Foundational is done:
   - Developer A: US2 (Tool/Skill writes)
   - Developer B: US4 (Prompt building)
   - Developer C: US5 (Response building)
3. After US2/4/5: Developer A does US3 (Subagent)
4. After US1: Developer B does US6 (Audit)
5. Team merges for Phase 9 (Integration)

---

## Notes

- [P] tasks = different files, no dependencies on incomplete tasks
- [Story] label maps task to specific user story for traceability
- Each user story is independently completable and testable
- Verify tests fail before implementing (TDD per Constitution Principle II)
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- Soft limit threshold defaults are defined in `ContextConfig::default()`
- Category write locks use `Arc<RwLock<Vec<RecordEntry>>>` per category
- Sensitive data desensitization is upstream responsibility (Context does not handle)
- Context is destroyed immediately on execution interruption (no recovery)
