# API Contract Quality Checklist: Agent Hook 配置管理

**Purpose**: Validate that Hook REST API endpoint requirements are complete, clear, consistent, and implementable by both frontend and backend teams
**Created**: 2026-06-02
**Feature**: [spec.md](../spec.md)

**Note**: This checklist tests the quality of the API CONTRACT REQUIREMENTS — not whether the endpoints work correctly.

---

## Endpoint Completeness

- [ ] CHK001 — Is the full URL path pattern documented for each Hook endpoint, including query parameters and path variables? [Completeness, Tasks T022/T023/T042]
- [ ] CHK002 — Are request body schemas (JSON shapes) specified for POST and PUT endpoints, distinguishing required vs optional fields? [Completeness, Spec §FR-004]
- [ ] CHK003 — Are response body schemas specified for all success (200/201) and error responses? [Completeness, Gap]
- [ ] CHK004 — Is the `POST /api/agents/:id/hooks` endpoint's behavior for `action_params` specified per action type — what fields are mandatory for `call_function` vs `call_workflow` vs `http_webhook`? [Clarity, Spec §FR-004]
- [ ] CHK005 — Are the `GET /api/agents/:id/hooks/executions` query parameters (agent_id, trigger_point, outcome, from, to, page, page_size) fully enumerated with types and defaults? [Completeness, Spec §FR-015]

## Response Format Consistency

- [ ] CHK006 — Is the response envelope format (`{ code, message, data }`) consistent with existing 003/004 API conventions across all Hook endpoints? [Consistency, Spec §FR vs Plan §Error Codes]
- [ ] CHK007 — Does the `GET /api/agents/:id/hooks` response include resolved reference names (Function name, Workflow name) per T047, and is this documented in the API contract? [Consistency, Tasks T047]
- [ ] CHK008 — Is the `PUT` response format documented — does it return the updated Hook object or just a success acknowledgment? [Clarity, Gap]
- [ ] CHK009 — Is the `DELETE` response format documented — what is returned on success vs on hook-not-found (6006)? [Clarity, Gap]

## Error Handling Contract

- [ ] CHK010 — Are all 6 error codes (6001-6006) documented with their HTTP status codes, trigger conditions, and user-facing messages? [Completeness, Plan §Error Codes]
- [ ] CHK011 — Is the optimistic lock conflict response (4094) for Hook updates specified with both the HTTP status and the body containing the server's current `updated_at`? [Clarity, Tasks T010]
- [ ] CHK012 — Is the error response format consistent between validation errors (400-level, e.g., 6002/6003) and server errors (500-level, e.g., 6005)? [Consistency]
- [ ] CHK013 — Are the HTTP status codes for Hook-specific errors (6001=422, 6003=400, 6004=408) correctly mapped and consistent with the existing `http_status_for_code()` convention? [Consistency, Plan §Error Codes]

## Pagination & Query Contract

- [ ] CHK014 — Is the pagination format for Hook execution history (`GET /api/agents/:id/hooks/executions`) specified — `page`, `page_size`, `total` in response? [Completeness, Spec §FR-015 / Tasks T041]
- [ ] CHK015 — Is the default page size and maximum page size documented for execution history? Missing these could cause unbounded queries. [Clarity, Gap]
- [ ] CHK016 — Are the `from` and `to` time range parameters' format (ISO 8601? Unix timestamp?) and timezone handling specified? [Clarity, Spec §FR-015]

## Authentication & Authorization Contract

- [ ] CHK017 — Is the JWT authentication requirement documented for all Hook CRUD endpoints — which middleware is applied and what claims are extracted? [Completeness, Tasks T023]
- [ ] CHK018 — Are the RBAC rules for Hook execution history (FR-019: non-Super limited to own sessions) specified in terms of how the session-ownership check is performed — by matching `admin_id` from JWT to `chat_sessions.admin_id`? [Clarity, Spec §FR-019]
- [ ] CHK019 — Is the behavior documented for the main Agent Hook UI (FR-017: "不渲染") — does the API also reject non-Super requests to main Agent Hook endpoints, or only hide the UI? [Clarity, Spec §FR-017 vs Tasks T032]

## Data Model ↔ API Alignment

- [ ] CHK020 — Does the `AgentHook` response schema in the API contract include `blocking_mode` and `timeout_ms` fields, or are these documented separately? [Completeness, Spec §FR-010/FR-011 vs Tasks T007]
- [ ] CHK021 — Is the `action_params` JSON structure documented for each action type in the API contract, or left as an opaque blob for the client to discover? [Clarity, Gap]
- [ ] CHK022 — Does the `HookExecution` response schema include `agent_identifier` (audit snapshot from T004), and is the behavior when agent is deleted documented ("已删除的 Agent {identifier}")? [Completeness, Spec US4 Acceptance Scenario 3]

## API Change & Versioning

- [ ] CHK023 — Since Hook endpoints are new resources under `/api/agents/:id/hooks`, is there a documented API versioning strategy — are these v1 endpoints and how will breaking changes be handled? [Gap, Versioning]
- [ ] CHK024 — Are the Hook endpoints included in any API documentation generation or OpenAPI spec — is this requirement specified? [Gap, Documentation]

---

## Notes

- Items CHK001–CHK005 focus on endpoint specification completeness
- Items CHK006–CHK009 focus on response format consistency with existing conventions
- Items CHK010–CHK013 focus on error handling contract quality
- Items CHK014–CHK016 focus on pagination and query parameter specification
- Items CHK017–CHK019 focus on auth/RBAC contract clarity
- Items CHK020–CHK024 focus on data model alignment and versioning
