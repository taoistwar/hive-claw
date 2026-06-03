# API Contract Completeness Checklist: Agent Context 能力设计

**Purpose**: Verify that every Functional Requirement (FR-001 ~ FR-021) and Integration Requirement (IL-001 ~ IL-004) in the spec has a corresponding, unambiguous method signature or behavioral contract in `contracts/api.md`.
**Created**: 2026-06-02
**Feature**: [spec.md](file:///home/developer/agent/hive-claw/specs/009-agent-context/spec.md)
**Contract**: [contracts/api.md](file:///home/developer/agent/hive-claw/specs/009-agent-context/contracts/api.md)

## Core CRUD Operations Coverage

- [ ] CHK030 Does the `AgentContext::new()` signature accept all three required parameters (context_id, user_input, config) per FR-001 and FR-002? [Completeness, FR-001/FR-002, contracts/api.md §Creation]
- [ ] CHK031 Is `AgentContext::user_input()` return type `&UserInput` sufficient for FR-002's requirement of storing "user raw input, current message, session info, request metadata"? [Completeness, FR-002, data-model.md §UserInput]
- [ ] CHK032 Does `AgentContext::set_record()` accept all five parameters (category, key, value, source, iteration) needed for FR-003's "entities, intentions, tool results, query results, workflow results, reasoning results"? [Completeness, FR-003, contracts/api.md §Write Operations]
- [ ] CHK033 Is the `Category` enum in contracts/api.md consistent with the six categories listed in FR-003 + FR-009 (extensions)? [Consistency, FR-003/FR-009, data-model.md §Category]
- [ ] CHK034 Does `AgentContext::get_record()` and `AgentContext::get_category()` together satisfy FR-014's "query by category" requirement? [Completeness, FR-014, contracts/api.md §Read Operations]

## Tool/Skill Write Operations

- [ ] CHK035 Does `AgentContext::set_record()` contract explicitly document that it records a StateChangeLog on overwrite, per FR-011? [Clarity, FR-005/FR-011, contracts/api.md §Write Operations]
- [ ] CHK036 Is `AgentContext::append_record()` return type `Result<(), ContextError>` sufficient to report soft-limit-exceeded warnings, per Edge Case §soft limit? [Ambiguity, FR-005]
- [ ] CHK037 Is there a dedicated method or mechanism for Tool/Skill to record `ToolCallRecord` and `SkillExecutionRecord` (FR-011), or is this implicit through `set_record()`? [Gap, FR-011]

## Sub-Agent Fork & Merge

- [ ] CHK038 Does `fork_for_subagent()` accept a `blacklisted_categories` parameter and apply it correctly, per FR-006's "default all-visible + blacklist" requirement? [Completeness, FR-006, contracts/api.md §Creation]
- [ ] CHK039 Is the blacklist write guard behavior (returning `ContextError::BlacklistedCategoryWrite`) documented in the `fork_for_subagent()` contract, or only in the `ContextError` enum? [Clarity, FR-006, contracts/api.md §ContextError]
- [ ] CHK040 Does `merge_subagent_context()` contract specify the "main priority + child namespace prefix" rule explicitly, per FR-006? [Clarity, FR-006, contracts/api.md §Merge Operations]
- [ ] CHK041 Does the contract specify the nested delegation prefix format (`sub-001/sub-002/{category}/`) per FR-018, or only the single-level format (`subagent_id/`)? [Completeness, FR-018, contracts/api.md §Merge Operations]
- [ ] CHK042 Is there a contract requirement that `fork_for_subagent()` rejects blacklisting the `UserInput` category (per data-model.md §Validation Rules)? [Gap, data-model.md §Validation Rules]

## Prompt Building

- [ ] CHK043 Does `build_prompt_budget(max_tokens)` return type `Result<PromptData, ContextError>` match FR-007's "extract structured data + truncate by priority" requirement? [Completeness, FR-007, contracts/api.md §Prompt Building]
- [ ] CHK044 Is the fixed priority truncation order (user_input > recent_reasoning > tool_results > entities > history_state) documented in the `build_prompt_budget()` contract or only in its doc comment? [Clarity, FR-007, contracts/api.md §PromptData]
- [ ] CHK045 Does `PromptData` struct include a `truncated: bool` field to signal whether truncation occurred, per SC-004's measurability requirement? [Completeness, SC-004, contracts/api.md §PromptData]

## Response Building & Extensions

- [ ] CHK046 Does `set_response_payload()` accept `ResponsePayload` with all required fields (text, extensions, object_refs, suggestions) per FR-008 and FR-009? [Completeness, FR-008/FR-009, contracts/api.md §ResponsePayload]
- [ ] CHK047 Does the contract provide a method to retrieve the response payload (e.g., `get_response_payload()`), or is it only set-able? [Gap, FR-008]
- [ ] CHK048 Is `ExtensionType` enum (Card, Image, Suggestion, Link, Button, Table, Chart, ObjectRef) documented in contracts/api.md or only in data-model.md? [Completeness, FR-009, data-model.md §ExtensionContent]
- [ ] CHK049 Is there a contract for `ExtensionContent` type validation per the Edge Case "Extensions 动态扩展内容的类型校验与 schema 验证"? [Gap, Edge Cases §Extensions]

## Lifecycle & State Machine

- [ ] CHK050 Does `set_lifecycle_state()` contract specify which transitions are valid (Active→Completed, Active→Terminated, Active→Merging→Active) and which return `IllegalStateTransition`? [Completeness, FR-015, data-model.md §State Transitions]
- [ ] CHK051 Does the contract specify that `set_record()` and `append_record()` return `ContextError::IllegalStateTransition` when Context is in Completed or Terminated state, per FR-017? [Completeness, FR-017, contracts/api.md §Write Operations]
- [ ] CHK052 Does the contract specify behavior of writes during Merging state (only merge operations allowed), per data-model.md §State Transitions? [Completeness, data-model.md §State Transitions]

## Thread Safety & Concurrency

- [ ] CHK053 Does the contract document the per-category `Arc<RwLock<T>>` concurrency model, or is this an internal implementation detail? [Clarity, FR-012, plan.md §Phase 0]
- [ ] CHK054 Is the `ReadView` struct guaranteed to be `Sync` + `Send` for concurrent access across threads? [Completeness, FR-013, SC-005]
- [ ] CHK055 Does the contract specify that `AgentContext` itself is `Sync` + `Send` (via `Arc` interior mutability)? [Completeness, FR-012, SC-005]

## Serialization & Schema

- [ ] CHK056 Does `to_json()` contract specify that the output includes `schema_version` field per FR-020? [Completeness, FR-020, contracts/api.md §Serialization]
- [ ] CHK057 Does `from_json()` contract specify that it rejects mismatched `schema_version` with a specific error type? [Completeness, FR-020, contracts/api.md §Serialization]
- [ ] CHK058 Is `ContextSnapshot` struct defined in contracts/api.md, or only in data-model.md? [Completeness, FR-016, data-model.md §ContextSnapshot]

## Sensitive Data Guard

- [ ] CHK059 Does `set_record()` and `append_record()` contract explicitly list `ContextError::RejectedSensitiveData` as a possible return value per FR-021? [Completeness, FR-021, contracts/api.md §Write Operations / ContextError]
- [ ] CHK060 Is the `SENSITIVE_FIELD_PATTERNS` constant list documented in the contract or only in the implementation notes? [Clarity, FR-021, contracts/api.md §Sensitive Data Guard]
- [ ] CHK061 Does the contract specify whether sensitive data check applies to the JSON value content (e.g., nested keys in `serde_json::Value`), or only to the top-level key string? [Ambiguity, FR-021]

## Hook Integration

- [ ] CHK062 Does `AgentContextSyncHook` contract specify `before_subagent_fork()` and `after_subagent_merge()` hooks (per tasks.md T055), or only `before_llm_call()`, `after_tool_execution()`, `after_iteration()`? [Completeness, tasks.md T055, contracts/api.md §AgentContextSyncHook]
- [ ] CHK063 Does the contract specify that hook failures are logged as error but do not abort the AgentLoop, per FR-019? [Completeness, FR-019, contracts/api.md §AgentContextSyncHook]
- [ ] CHK064 Does the contract specify the hook execution order relative to other hooks (FR-019: "after other AgentHooks"), per IL-002? [Completeness, IL-002, contracts/api.md §AgentContextSyncHook]

## Integration-Level Requirements

- [ ] CHK065 Are IL-001 (AgentContext created before TurnContext) and IL-003 (Arc release on panic) documented as API contracts or as implementation constraints in plan.md? [Completeness, IL-001/IL-003, plan.md §Phase 9]
- [ ] CHK066 Is IL-004 (debug_mode via ContextConfig) reflected in the `ContextConfig` struct in contracts/api.md? [Completeness, IL-004, contracts/api.md §ContextConfig]

## Error Handling Completeness

- [ ] CHK067 Does `ContextError` enum cover all error scenarios defined across FR-001 ~ FR-021 (CategoryNotFound, RecordNotFound, BlacklistedCategoryWrite, IllegalStateTransition, MergeFailed, RejectedSensitiveData)? [Completeness, FR-006/FR-017/FR-021, contracts/api.md §ContextError]
- [ ] CHK068 Is there a `ContextError` variant for schema version mismatch (FR-020), or should `MergeFailed` be reused? [Ambiguity, FR-020]
- [ ] CHK069 Is there a `ContextError` variant for soft-limit-exceeded conditions, or is this only a warning log per Edge Case §soft limit? [Clarity, Edge Cases §soft limit]

## Notes

- This checklist verifies that every spec requirement (25 FR/IL items) maps to a concrete method signature, struct, or behavioral contract in `contracts/api.md`.
- **Traceability**: 40/40 items (100%) include spec/FR/IL/SC references.
- **Resolved gaps** (2026-06-02):
  - CHK037 → Added `record_tool_call()` and `record_skill_execution()` dedicated APIs
  - CHK042 → Added `InvalidBlacklist` error variant + fork_for_subagent constraint documentation
  - CHK047 → Added `get_response_payload()` read accessor
  - CHK048 → Added `ExtensionContent` + `ExtensionType` enum with validation constraints
  - CHK049 → Added ExtensionContent type validation constraint documentation
  - CHK058 → Added `ContextSnapshot` struct with thread-safety constraint
  - CHK062 → Added `before_subagent_fork()` and `after_subagent_merge()` hook methods
  - CHK068 → Added `SchemaVersionMismatch` error variant
- **Remaining ambiguities** (3 items):
  - CHK036 — append_record soft limit return behavior (whether to return Ok with side-effect log or new error variant)
  - CHK053 — concurrency model documented in Overview section but per-category lock implementation details remain internal
  - CHK069 — soft limit exceeded does not produce error variant (by design: warn! only); confirmed acceptable per Edge Case §soft limit
- Items numbered CHK030-CHK069 to continue after `concurrency.md` (CHK001-CHK029).
