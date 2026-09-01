# API Contract Quality Checklist: Agent Hook 配置管理

**Purpose**: Validate that Hook REST API endpoint requirements are complete, clear, consistent, and implementable by both frontend and backend teams
**Created**: 2026-06-02
**Revalidated**: 2026-07-24（执行结果 tracing-only；内部 AgentContext updates 合同已同步）
**Feature**: [spec.md](../spec.md)

**Note**: This checklist tests the quality of the API CONTRACT REQUIREMENTS — not whether the endpoints work correctly.

---

## Endpoint Completeness

- [ ] CHK001 — Is the full URL path pattern documented for each Hook endpoint, including query parameters and path variables? [Completeness, Tasks T022/T023/T042]
- [ ] CHK002 — Are request body schemas (JSON shapes) specified for POST and PUT endpoints, distinguishing required vs optional fields? [Completeness, Spec §FR-004]
- [ ] CHK003 — Are response body schemas specified for all success (200/201) and error responses? [Completeness, Gap]
- [ ] CHK004 — Is the `POST /api/agents/:id/hooks` endpoint's behavior for `action_params` specified per action type — what fields are mandatory for `call_function` vs `call_workflow` vs `http_webhook`? [Clarity, Spec §FR-004]
- [x] CHK005 — Does the contract明确排除 `GET /api/agents/:id/hooks/executions` 及任何全局执行历史查询 API? [Completeness, Spec §FR-015] → **已验证**: contract 仅保留 Hook 配置 CRUD

## Response Format Consistency

- [ ] CHK006 — Is the response envelope format (`{ code, message, data }`) consistent with existing 003/004 API conventions across all Hook endpoints? [Consistency, Spec §FR vs Plan §Error Codes]
- [ ] CHK007 — Does the `GET /api/agents/:id/hooks` response include resolved reference names (Function name, Workflow name) per T047, and is this documented in the API contract? [Consistency, Tasks T047]
- [ ] CHK008 — Is the `PUT` response format documented — does it return the updated Hook object or just a success acknowledgment? [Clarity, Gap]
- [ ] CHK009 — Is the `DELETE` response format documented — what is returned on success vs on hook-not-found (6006)? [Clarity, Gap]

## Error Handling Contract

- [ ] CHK010 — Are all 6 error codes (6001-6006) documented with their HTTP status codes, trigger conditions, and user-facing messages? [Completeness, Plan §Error Codes]
- [ ] CHK011 — Is the optimistic lock conflict response (4094) for Hook updates specified with both the HTTP status and the body containing the server's current `updated_at`? [Clarity, Tasks T010]
- [x] CHK012 — Is the error response format consistent between validation errors (400-level, e.g., 6002/6003) and server errors (500-level, e.g., 6005)? [Consistency] → **已解决**: 均使用 `{code,message}` JSON；运行时 message 为普通文本，不嵌套 JSON
- [x] CHK013 — Are the HTTP status codes for Hook-specific errors (6001=422, 6003=400, 6004=408) correctly mapped and consistent with the existing `http_status_for_code()` convention? [Consistency, Plan §Error Codes] → **已验证**: 6004=408、6005=500，并由非 ignored 响应测试覆盖
- [x] CHK025 — Does the synchronous Assistant error contract preserve the original terminal error when `on_agent_error` fails or exhausts its phase budget, without a second nested error response? [Consistency, Contract §Agent 执行错误] → **已解决**: Hook 快照加载后的终止错误共享 30 秒阶段总预算，失败或耗尽不覆盖原始 `{code,message}`
- [x] CHK026 — Do successful Hook POST / PUT / DELETE operations invalidate the corresponding AgentContent cache while preserving the successful response if invalidation fails? [Consistency, Contract §Hook CRUD] → **已解决**: 三个 mutation 仅在 DB 成功后 best-effort 失效；失败输出固定 `redis_delete_failed` tracing

## Removed Execution-History Contract

- [x] CHK014 — Is execution-history pagination absent because no execution-history query endpoint exists? [Consistency, Spec §FR-015] → **已验证**
- [x] CHK015 — Are obsolete execution-history page-size defaults removed from the model and API contract? [Consistency, Spec §FR-015] → **已验证**
- [x] CHK016 — Are obsolete execution-history time-range filters removed from the API contract? [Consistency, Spec §FR-015] → **已验证**

## Authentication & Authorization Contract

- [ ] CHK017 — Is the JWT authentication requirement documented for all Hook CRUD endpoints — which middleware is applied and what claims are extracted? [Completeness, Tasks T023]
- [x] CHK018 — Is execution-history access unavailable to every role because the API is removed? [Clarity, Spec §FR-015] → **已验证**
- [ ] CHK019 — Is the behavior documented for the main Agent Hook UI (FR-017: "不渲染") — does the API also reject non-Super requests to main Agent Hook endpoints, or only hide the UI? [Clarity, Spec §FR-017 vs Tasks T032]

## Data Model ↔ API Alignment

- [ ] CHK020 — Does the `AgentHook` response schema in the API contract include `blocking_mode` and `timeout_ms` fields, or are these documented separately? [Completeness, Spec §FR-010/FR-011 vs Tasks T007]
- [ ] CHK021 — Is the `action_params` JSON structure documented for each action type in the API contract, or left as an opaque blob for the client to discover? [Clarity, Gap]
- [x] CHK022 — Is the obsolete `HookExecution` response schema absent from the contract? [Completeness, Spec §FR-015] → **已验证**
- [x] CHK027 — Is `_agent_context_updates` explicitly documented as an internal Function/Workflow result contract rather than a Hook CRUD request field, with no new persistence API or audit-mode override? [Boundary, Contract §Hook action AgentContext] → **已解决**

## API Change & Versioning

- [ ] CHK023 — Since Hook endpoints are new resources under `/api/agents/:id/hooks`, is there a documented API versioning strategy — are these v1 endpoints and how will breaking changes be handled? [Gap, Versioning]
- [ ] CHK024 — Are the Hook endpoints included in any API documentation generation or OpenAPI spec — is this requirement specified? [Gap, Documentation]

---

## Notes

- Items CHK001–CHK005 focus on endpoint specification completeness
- Items CHK006–CHK009 focus on response format consistency with existing conventions
- Items CHK010–CHK013 and CHK025–CHK026 focus on error handling contract quality
- Items CHK014–CHK016 verify removal of the execution-history query contract
- Items CHK017–CHK019 focus on auth/RBAC contract clarity
- Items CHK020–CHK024 and CHK027 focus on data model alignment and versioning
