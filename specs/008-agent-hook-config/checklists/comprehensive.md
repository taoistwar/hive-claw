# Comprehensive Quality Checklist: Agent Hook 配置管理

**Purpose**: Validate requirement completeness, clarity, consistency, and measurability across security, reliability, audit, and UX dimensions
**Created**: 2026-06-02
**Revalidated**: 2026-07-24（Hook sticky tracing-only + 受控 AgentContext updates）
**Feature**: [spec.md](../spec.md)

**Note**: This checklist tests the quality of the REQUIREMENTS themselves — not whether the implementation works correctly. Each item asks whether a requirement aspect is properly specified, clear, consistent, or measurable.

---

## Security & Boundary Requirements Quality

- [ ] CHK001 — Are the SSRF protection rules for `http_webhook` explicitly aligned with existing 004-agent-runtime FR-001 SSRF rules (same IP ranges, same metadata endpoints)? [Consistency, Spec §FR-007 / 004 Spec §FR-001]
- [ ] CHK002 — Is the scope of `https://` enforcement clearly defined — does it also reject `https://` URLs that resolve to internal IPs via DNS rebinding? [Clarity, Spec §FR-007]
- [ ] CHK003 — Are the `action_params` for `http_webhook` hooks subject to the same SSRF check at save-time (URL validation) AND at execution-time (DNS resolution check), or only one boundary? [Completeness, Spec §FR-007 / Spec §FR-007a]
- [x] CHK004 — Are RBAC requirements limited to Hook configuration CRUD, with no execution-history viewing permission or handler remaining? [Consistency, Spec §FR-015/FR-017–FR-018] → **已验证**
- [ ] CHK005 — Does the spec define what happens when a Super user demotes themselves mid-session while hook execution is in flight (TOCTOU for permission changes)? [Coverage, Edge Case Gap]
- [x] CHK006 — Are the `action_params.headers` for webhook hooks subject to injection validation — could a System+ admin inject `\r\n` to perform HTTP header injection? [Completeness, Gap] → **已解决**: FR-007b 新增 header name/value 校验规则
- [ ] CHK007 — Is the `chat_respond` Function exclusion for Hook calls clearly justified in the spec, and are there any other builtin Functions that should similarly be excluded? [Clarity, Spec Assumptions]
- [ ] CHK008 — Does the spec define whether Hook-triggered Function/Workflow calls are subject to the same capability-based authorization as direct agent tool calls? [Clarity, Spec §FR-013]

---

## Reliability & Failure Mode Requirements Quality

- [ ] CHK009 — Are the 7 trigger point insertion locations in tasks.md (T028) consistent with the spec's lifecycle order (FR-002)? Specifically, `before_llm_call` appears after `before_agent_start` in both documents? [Consistency, Spec §FR-002 vs Tasks T028]
- [x] CHK010 — Is the blocking/non-blocking mode behavior consistently defined across FR-009, FR-010, FR-012, and the Edge Cases? Specifically: does blocking mode abort ONLY the current trigger-point's hook chain, or the ENTIRE agent session? [Consistency, Spec §FR-009/FR-010/FR-012 vs Edge Cases] → **已解决**: 阻塞模式失败中止整个当前 Agent 请求，跳过正常 finalize；非阻塞模式仅记录 tracing 后继续
- [ ] CHK011 — Is the timeout interaction between Hook execution (FR-011, default 10s) and the orchestrator's LLM timeout (30s, per 004 spec) clearly defined — does a blocking-mode timeout count toward the agent's total execution budget? [Clarity, Spec §FR-011 vs Plan Hook Execution Flow]
- [ ] CHK012 — Are the webhook retry semantics (FR-007a: 3 retries, 1s/2s/4s) consistent with the edge case statement that "HTTP Webhook 超时不计入重试窗口（每次重试独立计时）"? Does this mean each retry gets a fresh 10s timeout? [Clarity, Spec §FR-007a vs Edge Cases]
- [x] CHK013 — Is the `on_agent_error` hook guaranteed to execute when the first `fetch_content()` fails before hooks are loaded? [Completeness, Spec §FR-002 vs Tasks T028] → **已解决**: 首次 Agent 内容加载失败且 Hook 快照不存在时明确跳过；已有最近一次成功快照的后续加载失败仍触发
- [x] CHK014 — Does the spec define the expected behavior when a blocking-mode hook times out vs when it returns an explicit error? [Clarity, Spec §FR-010 vs Plan Error Codes] → **已解决**: `POST /api/assistant` 同步 JSON 中超时为 6004/HTTP 408，其他阻塞失败为 6005/HTTP 500；不恢复 SSE 契约
- [x] CHK015 — Is the requirement for "skip or abort" semantics (Edge Cases) now resolved by the clarify session (Q3: always continue subsequent hooks in non-blocking mode)? The Edge Cases text should no longer mention variable strategies. [Consistency, Spec Clarifications vs Edge Cases] → **已验证**: Edge Cases 已更新为明确规则
- [x] CHK036 — Are all terminal Agent errors after Hook snapshot loading required to enter one `on_agent_error` path, including preset lookup, Provider, blocking Hook, route loop, max-hop and later Agent-content failures? [Completeness, Spec §FR-002/SC-008] → **已解决**: spec、plan、tasks 与 contract 列出统一终止错误覆盖范围
- [x] CHK037 — Is `on_agent_error` bounded as one phase rather than by an independent budget per Hook, and are timeout/failure replacement and recursion explicitly forbidden? [Reliability, Spec §FR-011/SC-008] → **已解决**: 固定 30 秒阶段总预算；耗尽只记录脱敏 tracing 并保留原始错误
- [x] CHK038 — Is Hook snapshot lifetime defined per successful Agent hop, and do successful Hook mutations invalidate the shared AgentContent cache without replacing a committed result on invalidation failure? [Consistency, Spec §FR-019/FR-020] → **已解决**: hop 内固定、下一 hop 重新 fetch；后续失败使用最近成功快照，首次失败跳过；CRUD 成功后 best-effort 失效并使用静态错误分类

---

## Audit & Observability Requirements Quality

- [ ] CHK016 — Are all four outcome states (success / error / timeout / skipped) defined in the spec with clear trigger conditions for each? For example, when exactly is `skipped` used vs `error`? [Completeness, Spec §FR-014 / Key Entities]
- [x] CHK017 — Does FR-016 require a bounded `error_kind` instead of arbitrary downstream error text? [Security, Spec §FR-016] → **已验证**
- [x] CHK018 — Does the spec forbid recording Webhook URL, request/response body and full user messages in tracing? [Completeness, Spec §FR-016] → **已验证**
- [x] CHK019 — Is retention configuration explicitly unnecessary because Hook execution history is never written? [Consistency, Spec §FR-015] → **已验证**
- [x] CHK020 — Is persistence of `context_snapshot` explicitly forbidden? [Clarity, Spec §FR-015] → **已验证**
- [x] CHK021 — Is `request_id` propagated in structured tracing consistently with existing runtime events? [Consistency, Spec §FR-014] → **已验证**
- [x] CHK022 — Is the database INSERT failure case eliminated by forbidding all Hook execution-history INSERTs? [Completeness, Spec §FR-015] → **已验证**

---

## UX & API Contract Requirements Quality

- [ ] CHK023 — Are all 6 error codes (6001-6006) mapped to user-facing Chinese messages, and are those messages actionable (identifying cause + suggesting next step) per Constitution Principle III? [Completeness, Plan §Error Codes]
- [ ] CHK024 — Is the optimistic lock semantics for Hook updates clearly specified — does the client receive `updated_at` on GET and must send it back on PUT, returning 4094 on conflict? [Clarity, Spec §FR-004 vs Tasks T010]
- [x] CHK025 — Are execution-history pagination requirements absent because the history API is removed? [Consistency, Spec §FR-015] → **已验证**
- [ ] CHK026 — Are the Hook sort_order uniqueness semantics clearly defined — can two hooks share the same sort_order within a trigger_point, or must they be unique? [Clarity, Tasks T003 Unique Index]
- [ ] CHK027 — Does the spec define what the frontend should display when a Hook's referenced Function/Workflow has been deleted but the Hook config still exists (Edge Case "Hook 动作引用失效")? [Completeness, Spec Edge Cases]
- [ ] CHK028 — Are accessibility requirements (keyboard navigation, ARIA labels, focus management) specified for the AgentHookEditor and HookFormModal components? [Gap, Accessibility]
- [ ] CHK029 — Is the main Agent Hook editing area "不渲染" behavior (FR-017) clearly distinguishable from "rendered but disabled" for non-Super viewing non-main agents? [Clarity, Spec §FR-017 / Tasks T032]
- [ ] CHK030 — Are requirements defined for the empty state of the Hook list (no hooks configured) — should it show a placeholder CTA or remain blank? [Completeness, Gap in US1 Acceptance Scenarios]

---

## Cross-Cutting Requirement Quality

- [ ] CHK031 — Do the 9 edge cases in the spec all have corresponding coverage in the tasks? Specifically, "Hook 配置更新时机" (snapshot at session start) — which task implements this? [Traceability, Spec Edge Cases → Tasks]
- [x] CHK032 — Is the historical "Hook 为严格只读观察者" clarification explicitly superseded and consistently replaced by the controlled `_agent_context_updates` contract across US3, requirements, plan and assumptions? [Consistency, Spec §Clarifications/US3/FR-024–FR-026/Assumptions] → **已解决**: 只读输入快照 + 固定 updates 通道；不直接修改持久化配置、system prompt、数据库或审计模式
- [x] CHK039 — Does the controlled update contract enumerate accepted records/extensions/metadata mappings, invalid-item behavior, capability authorization, in-memory lifetime and sticky tracing-only audit boundary? [Security/Completeness, Spec §FR-024–FR-026 / Contract §Hook action AgentContext] → **已解决**
- [x] CHK033 — Is the obsolete execution-history query latency target removed while the Hook scheduling target remains measurable? [Consistency, Spec SC-003/SC-006] → **已验证**
- [x] CHK034 — Is the V023 history table documented as legacy-only, with no new reads/writes and no destructive migration of existing data? [Consistency, Spec §FR-015 / Assumptions] → **已验证**
- [ ] CHK035 — Are the Hook `action_params` JSON schema requirements defined for each action type — what are the mandatory vs optional fields within `action_params` for `call_function`, `call_workflow`, and `http_webhook`? [Completeness, Spec §FR-004 / Key Entities]

---

## Notes

- Items CHK001–CHK008 focus on security boundary requirements quality
- Items CHK009–CHK015 and CHK036–CHK038 focus on reliability and failure mode coverage
- Items CHK016–CHK022 focus on tracing completeness, privacy and consistency
- Items CHK023–CHK030 focus on UX contract and API design quality
- Items CHK031–CHK035 and CHK039 cross-check spec/tasks/plan alignment and constitutional compliance
