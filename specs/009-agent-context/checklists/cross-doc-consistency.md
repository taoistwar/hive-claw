# Cross-Document Consistency Checklist: Agent Context 能力设计

**Purpose**: Verify that spec.md, data-model.md, contracts/api.md, plan.md, and tasks.md are internally consistent — entity names, method signatures, state transitions, error types, and category definitions match across all documents.
**Created**: 2026-06-02
**Feature**: [spec.md](file:///home/developer/agent/hive-claw/specs/009-agent-context/spec.md)

## Entity Name Consistency

- [ ] CHK070 Is the `AgentContext` struct field naming consistent across data-model.md (`id: String`) and contracts/api.md (constructor parameter `context_id: String`)? Should data-model.md use `context_id` instead of `id`? [Consistency, data-model.md §AgentContext vs contracts/api.md §Creation]
- [ ] CHK071 Does data-model.md's `audit_log: Vec<Record<Operation>>` match contracts/api.md's `get_audit_log() -> Vec<AuditRecord>` return type? Are `Record<Operation>` and `AuditRecord` the same type or is there a naming inconsistency? [Consistency, data-model.md §AgentContext vs contracts/api.md §Audit Operations]
- [ ] CHK072 Is `ContextSnapshot`'s `audit_log_snapshot: Vec<AuditRecord>` consistent with CHK071's type resolution? [Consistency, data-model.md §ContextSnapshot vs contracts/api.md §ContextSnapshot]

## Category Enum Consistency

- [ ] CHK073 Does data-model.md's Category enum (9 variants: Entities, Intentions, ToolResults, QueryResults, WorkflowResults, ReasoningResults, Extensions, StateChanges, SubagentResults) match all category references in spec.md FR-003 (entities, intentions, tool_results, query_results, workflow_results, reasoning_results)? [Consistency, data-model.md §Category vs Spec §FR-003]
- [ ] CHK074 Is the FR-014 "按类别查询" requirement satisfied by the 9 categories in data-model.md, or are there missing categories (e.g., UserInput as a queryable category)? [Completeness, Spec §FR-014 vs data-model.md §Category]
- [ ] CHK075 Does contracts/api.md's `get_category(category: Category)` return type `Vec<RecordEntry>` match data-model.md's CategoryStore records structure? [Consistency, contracts/api.md §Read Operations vs data-model.md §CategoryStore]

## Lifecycle State Machine Consistency

- [ ] CHK076 Are the lifecycle state names consistent: data-model.md uses `Active/Completed/Terminated/Merging` (PascalCase) while spec.md §Assumptions uses lowercase `active/completed/terminated/merging`? [Consistency, data-model.md §LifecycleState vs Spec §Assumptions]
- [ ] CHK077 Does the state transition diagram in data-model.md (Active→Completed, Active→Terminated, Active→Merging→Active) match the lifecycle write constraints in contracts/api.md Overview? [Consistency, data-model.md §State Transitions vs contracts/api.md Overview]
- [ ] CHK078 Does contracts/api.md's `Merging` state allow only `merge_subagent_context` (per Overview) match data-model.md's "Merging→仅允许 merge 操作" (per State Transitions)? [Consistency, contracts/api.md Overview vs data-model.md §State Transitions]
- [ ] CHK079 Is the debug_mode exception for Terminated state (snapshot allowed) documented in contracts/api.md Overview consistent with IL-004? [Consistency, contracts/api.md Overview vs Spec §IL-004]

## Error Type Consistency

- [ ] CHK080 Does contracts/api.md's `ContextError` enum (7 variants: CategoryNotFound, RecordNotFound, BlacklistedCategoryWrite, IllegalStateTransition, MergeFailed, RejectedSensitiveData, InvalidBlacklist, SchemaVersionMismatch) cover all error scenarios referenced in spec.md and data-model.md? [Completeness, contracts/api.md §ContextError vs Spec/data-model.md]
- [ ] CHK081 Is `ContextError::IllegalStateTransition` used consistently across spec.md FR-017, contracts/api.md Overview, and data-model.md §State Transitions for all invalid state transitions? [Consistency, Spec §FR-017 vs contracts/api.md vs data-model.md]
- [ ] CHK082 Does spec.md FR-021's `ContextError::RejectedSensitiveData` match contracts/api.md's `RejectedSensitiveData { field_name: String, reason: String }` in terms of expected behavior? [Consistency, Spec §FR-021 vs contracts/api.md §ContextError]

## Method Signature Consistency

- [ ] CHK083 Does `AgentContext::new(context_id, user_input, config)` in contracts/api.md match tasks.md T022's description ("constructor with user_input, config, category initialization")? Is `category initialization` an internal detail of `new()` or a separate step? [Consistency, contracts/api.md §Creation vs tasks.md T022]
- [ ] CHK084 Does `AgentContext::set_record()` in contracts/api.md (5 parameters: category, key, value, source, iteration) match tasks.md T024's description? [Consistency, contracts/api.md §Write Operations vs tasks.md T024]
- [ ] CHK085 Does tasks.md T039 reference `AuditLogger::record_tool_call()` / `record_skill_execution()` while contracts/api.md defines them as methods on `AgentContext` directly? Are these two different APIs or should T039 be updated? [Consistency, tasks.md T039 vs contracts/api.md §Audit Operations]
- [ ] CHK086 Does tasks.md T041 (`get_audit_log()`) match contracts/api.md's `get_audit_log() -> Vec<AuditRecord>` return type? [Consistency, tasks.md T041 vs contracts/api.md §Audit Operations]

## Prompt Building Consistency

- [ ] CHK087 Does `build_prompt_budget(max_tokens: usize)` in contracts/api.md use the same parameter name and type as spec.md FR-007's description? [Consistency, contracts/api.md §Prompt Building vs Spec §FR-007]
- [ ] CHK088 Is the truncation priority order consistent: spec.md §Clarifications Q2 says "用户输入 > 最近推理 > Tool 结果 > 实体 > 历史状态" while contracts/api.md §Prompt Building says "用户输入(100%) > 最近推理 > Tool结果 > 实体 > 历史状态" and PromptData struct uses `recent_reasoning`, `tool_results`, `entities`, `history_state`? [Consistency, Spec §Clarifications vs contracts/api.md]
- [ ] CHK089 Does `PromptData` struct in contracts/api.md (`user_input: String`, `recent_reasoning: Option<String>`) match spec.md FR-007's "用户输入、已识别实体、Tool 结果、历史推理结果" — is `user_input` the raw text or processed? [Ambiguity, contracts/api.md §PromptData vs Spec §FR-007]

## Sub-Agent Merge Consistency

- [ ] CHK090 Does the namespace prefix format `sub-001/sub-002/{category}/` in spec.md FR-018 and data-model.md §Relationships match contracts/api.md §Merge Operations? [Consistency, Spec §FR-018 vs data-model.md §Relationships vs contracts/api.md §Merge Operations]
- [ ] CHK091 Does the conflict storage format `{subagent_id}/{category}/conflict/` in spec.md §Edge Cases match data-model.md §Relationships and contracts/api.md §Merge Operations? [Consistency, Spec §Edge Cases vs data-model.md vs contracts/api.md]
- [ ] CHK092 Does tasks.md T052's description ("iterate child categories, apply main-priority rule, namespace child data with `{subagent_id}/` prefix") match the full delegation chain prefix format in FR-018? [Consistency, tasks.md T052 vs Spec §FR-018]

## Serialization Consistency

- [ ] CHK093 Does `to_json()` output include `schema_version` per spec.md FR-020, and is this field documented in ContextSnapshot (`schema_version: String`) in contracts/api.md? [Consistency, Spec §FR-020 vs contracts/api.md §ContextSnapshot]
- [ ] CHK094 Does `from_json(json: &str, expected_version: &str) -> Result<Self, ContextError>` in contracts/api.md match spec.md FR-020's "反序列化时若 schema_version 不匹配则返回明确的版本不兼容错误"? [Consistency, contracts/api.md §Serialization vs Spec §FR-020]

## Extension Content Consistency

- [ ] CHK095 Does `ExtensionContent` appear in both the top-level `AgentContext.extensions` field (data-model.md §AgentContext) and as a `Category::Extensions` store entry (data-model.md §Category)? Is this duplication intentional or a design inconsistency? [Ambiguity, data-model.md §AgentContext vs §Category]
- [ ] CHK096 Does `ExtensionType` enum (8 variants) in contracts/api.md match the content types listed in spec.md FR-008 (文本回答、卡片、图片、推荐问题、链接、按钮、表格、图表、业务对象引用)? Note: FR-008 lists 9 types but ExtensionType has 8 — is "文本回答" excluded because it's in ResponsePayload.text? [Consistency, Spec §FR-008 vs contracts/api.md §ExtensionType]

## Plan & Tasks Alignment

- [ ] CHK097 Does plan.md's Phase 0 decision "CategoryLock<T> wrapper around Arc<RwLock<T>>" match data-model.md's CategoryStore<T> structure? Is CategoryLock used internally by CategoryStore or is it a separate abstraction? [Consistency, plan.md §Phase 0 vs data-model.md §CategoryStore]
- [ ] CHK098 Does tasks.md Phase 9 (Integration with AgentLoop) reference modifying `AgentLoop` in `loop_.rs` and `hook.rs`, which are existing files — are these modifications documented as backward-compatible changes? [Completeness, tasks.md Phase 9 vs plan.md §Integration Strategy]
- [ ] CHK099 Does tasks.md T042's description ("extend existing hook.rs or new file") reflect a decision that should be finalized before implementation? [Ambiguity, tasks.md T042]

## Soft Limit Consistency

- [ ] CHK100 Does data-model.md's CategoryStore `soft_limit: usize` + `warning_emitted: bool` match spec.md Edge Case §软限制 ("不拒绝写入，但记录告警日志供运维关注") and contracts/api.md's `set_record()` which does not return a soft-limit-specific error? [Consistency, data-model.md §CategoryStore vs Spec §Edge Cases vs contracts/api.md §Write Operations]
- [ ] CHK101 Does spec.md Edge Case §软限制告警日志 ("MUST 包含以下字段：category 名称、当前条目数、阈值、写入来源组件名、迭代轮次") have a corresponding implementation location in tasks.md or contracts/api.md? [Completeness, Spec §Edge Cases vs tasks.md/contracts/api.md]

## Notes

- This checklist verifies cross-document consistency across 5 artifacts: spec.md, data-model.md, contracts/api.md, plan.md, tasks.md.
- **Traceability**: 32/32 items (100%) reference specific sections across multiple documents.
- **Resolved issues** (2026-06-02):
  - CHK070 → data-model.md `id` renamed to `context_id` to match contracts/api.md
  - CHK071 → data-model.md `Record<Operation>` renamed to `AuditRecord` (union enum of ToolCallRecord + SkillExecutionRecord + StateChangeLog)
  - CHK072 → ContextSnapshot now uses consistent `Vec<AuditRecord>`
  - CHK076 → Lifecycle state names unified to PascalCase across all documents
  - CHK077 → State transitions verified consistent between data-model.md and contracts/api.md Overview
  - CHK078 → Merging state constraint verified consistent
  - CHK079 → debug_mode Terminated exception verified consistent with IL-004
  - CHK080 → ContextError enum covers all scenarios (8 variants)
  - CHK081 → IllegalStateTransition usage verified consistent
  - CHK082 → RejectedSensitiveData verified consistent
  - CHK083 → AgentContext::new() parameters verified consistent
  - CHK084 → set_record() parameters verified consistent
  - CHK085 → AuditLogger clarified as internal helper; AgentContext delegates to it (tasks.md T039/T040 updated)
  - CHK086 → get_audit_log() return type verified consistent
  - CHK089 → PromptData.user_input clarification: raw text string extracted from UserInput (not processed)
  - CHK090 → Namespace prefix format verified consistent (FR-018 ↔ data-model.md ↔ contracts/api.md)
  - CHK091 → Conflict storage format verified consistent
  - CHK092 → tasks.md T052 updated to use full delegation chain prefix format; T053/T055 file paths corrected to `context/hook.rs`
  - CHK093 → schema_version verified consistent
  - CHK094 → from_json signature verified consistent with FR-020
  - CHK095 → Category::Extensions clarified as index view; actual data stored in AgentContext.extensions
  - CHK096 → ExtensionType 8 variants vs FR-008 9 types clarified (text answer → ResponsePayload.text)
  - CHK097 → CategoryStore now references CategoryLock<T> wrapper to match plan.md
  - CHK098 → Backward compatibility of existing hook.rs modifications noted
  - CHK099 → AgentContextSyncHook file location decided: new file `context/hook.rs`
- **Remaining items requiring verification during implementation** (5):
  - CHK087 → build_prompt_budget parameter name/type consistency (trivial, will be verified by compiler)
  - CHK088 → Truncation priority order consistency (documents agree, implementation will validate)
  - CHK095 → Category::Extensions index view implementation approach (deferred to implementation phase)
  - CHK100 → Soft limit implementation approach (CategoryStore fields consistent with Edge Case design)
  - CHK101 → Soft limit log field completeness (will be verified in T035 test implementation)
- Items numbered CHK070-CHK101 to continue after `api-contracts.md` (CHK030-CHK069).
