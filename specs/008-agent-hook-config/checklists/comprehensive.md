# Comprehensive Quality Checklist: Agent Hook 配置管理

**Purpose**: Validate requirement completeness, clarity, consistency, and measurability across security, reliability, audit, and UX dimensions
**Created**: 2026-06-02
**Feature**: [spec.md](../spec.md)

**Note**: This checklist tests the quality of the REQUIREMENTS themselves — not whether the implementation works correctly. Each item asks whether a requirement aspect is properly specified, clear, consistent, or measurable.

---

## Security & Boundary Requirements Quality

- [ ] CHK001 — Are the SSRF protection rules for `http_webhook` explicitly aligned with existing 004-agent-runtime FR-001 SSRF rules (same IP ranges, same metadata endpoints)? [Consistency, Spec §FR-007 / 004 Spec §FR-001]
- [ ] CHK002 — Is the scope of `https://` enforcement clearly defined — does it also reject `https://` URLs that resolve to internal IPs via DNS rebinding? [Clarity, Spec §FR-007]
- [ ] CHK003 — Are the `action_params` for `http_webhook` hooks subject to the same SSRF check at save-time (URL validation) AND at execution-time (DNS resolution check), or only one boundary? [Completeness, Spec §FR-007 / Spec §FR-007a]
- [ ] CHK004 — Are RBAC requirements consistent for Hook CRUD vs Hook execution history viewing? Spec §FR-017/018 define CRUD roles (Super/System+), but §FR-019 adds session-ownership rules for history — are both checks required on the same handler? [Consistency, Spec §FR-017–FR-019]
- [ ] CHK005 — Does the spec define what happens when a Super user demotes themselves mid-session while hook execution is in flight (TOCTOU for permission changes)? [Coverage, Edge Case Gap]
- [x] CHK006 — Are the `action_params.headers` for webhook hooks subject to injection validation — could a System+ admin inject `\r\n` to perform HTTP header injection? [Completeness, Gap] → **已解决**: FR-007b 新增 header name/value 校验规则
- [ ] CHK007 — Is the `chat.respond` Function exclusion for Hook calls clearly justified in the spec, and are there any other builtin Functions that should similarly be excluded? [Clarity, Spec Assumptions]
- [ ] CHK008 — Does the spec define whether Hook-triggered Function/Workflow calls are subject to the same capability-based authorization as direct agent tool calls? [Clarity, Spec §FR-013]

---

## Reliability & Failure Mode Requirements Quality

- [ ] CHK009 — Are the 7 trigger point insertion locations in tasks.md (T028) consistent with the spec's lifecycle order (FR-002)? Specifically, `before_llm_call` appears after `before_agent_start` in both documents? [Consistency, Spec §FR-002 vs Tasks T028]
- [ ] CHK010 — Is the blocking/non-blocking mode behavior consistently defined across FR-009, FR-010, FR-012, and the Edge Cases? Specifically: does blocking mode abort ONLY the current trigger-point's hook chain, or the ENTIRE agent session? [Consistency, Spec §FR-009/FR-010/FR-012 vs Edge Cases]
- [ ] CHK011 — Is the timeout interaction between Hook execution (FR-011, default 10s) and the orchestrator's LLM timeout (30s, per 004 spec) clearly defined — does a blocking-mode timeout count toward the agent's total execution budget? [Clarity, Spec §FR-011 vs Plan Hook Execution Flow]
- [ ] CHK012 — Are the webhook retry semantics (FR-007a: 3 retries, 1s/2s/4s) consistent with the edge case statement that "HTTP Webhook 超时不计入重试窗口（每次重试独立计时）"? Does this mean each retry gets a fresh 10s timeout? [Clarity, Spec §FR-007a vs Edge Cases]
- [x] CHK013 — Is the `on_agent_error` hook guaranteed to execute when `build_agent_context()` itself fails (before hooks are loaded)? Tasks.md T028 notes this should be skipped — is this documented as an intentional exclusion in the spec? [Completeness, Spec §FR-002 vs Tasks T028] → **已解决**: Edge Cases 新增 "on_agent_error 提前失败路径" 条目
- [ ] CHK014 — Does the spec define the expected behavior when a blocking-mode hook times out vs when it returns an explicit error? Are the SSE error codes different? [Clarity, Spec §FR-010 vs Plan Error Codes]
- [x] CHK015 — Is the requirement for "skip or abort" semantics (Edge Cases) now resolved by the clarify session (Q3: always continue subsequent hooks in non-blocking mode)? The Edge Cases text should no longer mention variable strategies. [Consistency, Spec Clarifications vs Edge Cases] → **已验证**: Edge Cases 已更新为明确规则

---

## Audit & Observability Requirements Quality

- [ ] CHK016 — Are all four outcome states (success / error / timeout / skipped) defined in the spec with clear trigger conditions for each? For example, when exactly is `skipped` used vs `error`? [Completeness, Spec §FR-014 / Key Entities]
- [ ] CHK017 — Is the `error_summary` masking rule (FR-016) consistent with the existing 004-agent-runtime `payload_summary` masking rules? Are the same field blacklists (password/secret/token/api_key/authorization) applied? [Consistency, Spec §FR-016 vs 004 Spec]
- [ ] CHK018 — Does the spec define whether webhook response bodies are stored in `error_summary` or `context_snapshot`? If stored, are they subject to masking? [Completeness, Gap]
- [x] CHK019 — Is the `hook_executions` retention policy (30 days, T046) explicitly documented in the spec's Success Criteria or Assumptions, or only in tasks? [Completeness, Gap in Spec] → **已解决**: Assumptions 新增保留策略说明
- [x] CHK020 — Does the `context_snapshot` JSON field have a defined schema in the spec, or is it left as an opaque blob? If opaque, is its max size bounded? [Clarity, Key Entities] → **已解决**: Key Entities 中 context_snapshot 标注 ≤ 4KB 上限
- [ ] CHK021 — Is the `request_id` propagation requirement (FR-014, T024) consistent with existing 003/004 tracing — using the same middleware-generated request_id that flows through audit logs? [Consistency, Spec §FR-014 vs Plan]
- [ ] CHK022 — Are requirements defined for what happens when the `hook_executions` INSERT itself fails (e.g., DB connection lost)? Does this block the hook, or is it fire-and-forget? [Completeness, Gap]

---

## UX & API Contract Requirements Quality

- [ ] CHK023 — Are all 6 error codes (6001-6006) mapped to user-facing Chinese messages, and are those messages actionable (identifying cause + suggesting next step) per Constitution Principle III? [Completeness, Plan §Error Codes]
- [ ] CHK024 — Is the optimistic lock semantics for Hook updates clearly specified — does the client receive `updated_at` on GET and must send it back on PUT, returning 4094 on conflict? [Clarity, Spec §FR-004 vs Tasks T010]
- [ ] CHK025 — Are pagination requirements specified for Hook execution history (FR-015 / SC-006)? Is there a default page size and a max page size? [Completeness, Spec §FR-015]
- [ ] CHK026 — Are the Hook sort_order uniqueness semantics clearly defined — can two hooks share the same sort_order within a trigger_point, or must they be unique? [Clarity, Tasks T003 Unique Index]
- [ ] CHK027 — Does the spec define what the frontend should display when a Hook's referenced Function/Workflow has been deleted but the Hook config still exists (Edge Case "Hook 动作引用失效")? [Completeness, Spec Edge Cases]
- [ ] CHK028 — Are accessibility requirements (keyboard navigation, ARIA labels, focus management) specified for the AgentHookEditor and HookFormModal components? [Gap, Accessibility]
- [ ] CHK029 — Is the main Agent Hook editing area "不渲染" behavior (FR-017) clearly distinguishable from "rendered but disabled" for non-Super viewing non-main agents? [Clarity, Spec §FR-017 / Tasks T032]
- [ ] CHK030 — Are requirements defined for the empty state of the Hook list (no hooks configured) — should it show a placeholder CTA or remain blank? [Completeness, Gap in US1 Acceptance Scenarios]

---

## Cross-Cutting Requirement Quality

- [ ] CHK031 — Do the 9 edge cases in the spec all have corresponding coverage in the tasks? Specifically, "Hook 配置更新时机" (snapshot at session start) — which task implements this? [Traceability, Spec Edge Cases → Tasks]
- [ ] CHK032 — Is the "Hook 为只读观察者" assumption (clarify Q4) consistently reflected across ALL relevant spec sections — US3, FR-002, FR-008, Assumptions? [Consistency, Spec §US3 / FR-002 / FR-008 / Assumptions]
- [ ] CHK033 — Are the performance targets (SC-003: ≤50ms scheduling; SC-006: ≤2s history query) consistent with Constitution IV's p95 < 200ms constraint? The SC-003 target is stricter — is this intentional and justified? [Consistency, Spec SC vs Constitution IV]
- [x] CHK034 — Does the `agent_hooks.agent_id ON DELETE CASCADE` (T003) conflict with the HookExecution agent_id "no FK, audit retention" strategy (T004)? Is the intent that Hook configs cascade-delete with Agent but execution history persists? [Consistency, Tasks T003 vs T004] → **已验证**: 设计意图明确 — 配置级联删除，执行历史审计保留。Assumptions 已文档化此策略
- [ ] CHK035 — Are the Hook `action_params` JSON schema requirements defined for each action type — what are the mandatory vs optional fields within `action_params` for `call_function`, `call_workflow`, and `http_webhook`? [Completeness, Spec §FR-004 / Key Entities]

---

## Notes

- Items CHK001–CHK008 focus on security boundary requirements quality
- Items CHK009–CHK015 focus on reliability and failure mode coverage
- Items CHK016–CHK022 focus on audit completeness and consistency
- Items CHK023–CHK030 focus on UX contract and API design quality
- Items CHK031–CHK035 cross-check spec/tasks/plan alignment and constitutional compliance
