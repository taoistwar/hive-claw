# API Contract Quality Checklist: 004-agent-runtime

**Purpose**: Validate that the HTTP API contracts (api.md + host-functions.md) are complete, consistent, and unambiguous — cross-referenced against spec.md, data-model.md, SECURITY.md, and perf-evidence.md
**Created**: 2026-05-29
**Feature**: [spec.md](file:///home/developer/agent/hive-claw/specs/004-agent-runtime/spec.md)
**Depth**: Formal (40+ items)
**Scope**: Cross-doc consistency across all feature documents

## Envelope & Pagination Consistency

- [ ] CHK001 — Is the response envelope `{ code, message, data }` format consistently documented in api.md §0 AND actually reflected in every endpoint's example response? [Consistency, api.md §0 vs all §1–10]
- [ ] CHK002 — Is the pagination contract (`offset`/`limit` default 0/20, max 100, empty-not-404) applied uniformly to ALL GET list endpoints, or are some endpoints missing pagination docs? [Completeness, api.md §0 vs §1 Capabilities / §5 Functions / §7 Tools / §10 Chat sessions]
- [ ] CHK003 — Is `code = 0` for success consistently applied in every example response, or are some examples using `200` / omitting `code`? [Consistency, api.md all sections]
- [ ] CHK004 — Does the api.md `data.params` placeholder replacement convention (CHK191) have corresponding `params` fields defined in every error response that uses placeholders? [Completeness, api.md §0 vs §Errors table]

## Auth & JWT Consistency

- [ ] CHK005 — Are the authentication requirements consistent between api.md header ("全部端点都需要管理中心 JWT") and the actual `/api/auth/login` endpoint which must be public? [Consistency, api.md header vs §Auth endpoints]
- [ ] CHK006 — Is the account lockout mechanism (5 failed attempts, 15-minute lock) documented in api.md's auth section AND cross-referenced with the error code table? [Completeness, api.md Auth section vs §Errors]
- [ ] CHK007 — Is the JWT role system (Normal=1 / System=2 / Super=3) documented in api.md and consistent with spec.md FR-022 (Super-only dangerous perms) and the Capability Coverage Matrix? [Consistency, api.md header vs spec.md FR-022 vs api.md Capability Coverage Matrix]
- [ ] CHK008 — Is `GET /api/auth/me` response schema documented (AdminPublic: id, phone, nickname, role, status) and consistent with the Admin model in data-model.md? [Consistency, api.md Auth vs data-model.md §Admin]

## Error Code Completeness

- [ ] CHK009 — Are ALL error codes used in api.md endpoint responses listed in the §Errors table with HTTP status, internal meaning, and zh-CN user message? [Completeness, api.md all sections vs §Errors]
- [ ] CHK010 — Is error code 4094 (OptimisticLockConflict) consistently applied to all entities that use optimistic locking (Plugin/Workflow/Tool/Agent/Skill) — or are some missing? [Completeness, api.md §5 Functions / §6 Workflows / §7 Tools / §8 Skills / §9 Agents vs §Errors 4094]
- [ ] CHK011 — Is error code 4093 (ResourceInUse) documented with resource_type and N parameters for ALL entities that can block deletion (Plugin/Workflow/Function/Agent/Tag)? [Completeness, api.md §3 Tags / §4 Plugins / §5 Functions / §6 Workflows / §9 Agents vs §Errors 4093]
- [ ] CHK012 — Are the error codes in host-functions.md §3 (4030/4045/4001/4081/5000) a subset of the api.md §Errors table, or are there host-only codes not documented in api.md? [Consistency, host-functions.md §3 vs api.md §Errors]
- [ ] CHK013 — Is the `4081` timeout code in host-functions.md consistent with `5004` Plugin invocation timeout in api.md — or are these two different timeout scenarios that need clarification? [Clarity, host-functions.md §3 vs api.md §Errors 5004]
- [ ] CHK014 — Does the SSE `error` event format (`{ "code": 5003, "message": "..." }`) match the standard error envelope convention, or is it a divergent format? [Consistency, api.md §10 Chat SSE error vs api.md §0 envelope]

## Endpoint Coverage

- [ ] CHK015 — Does spec.md list ALL functional requirements that should have corresponding API endpoints, or are some FRs missing endpoint documentation? [Completeness, spec.md FR-001..FR-034 vs api.md §1–10]
- [ ] CHK016 — Is the `POST /api/functions/:id/invoke` test endpoint documented in api.md, including its request/response schema and required role? [Completeness, spec.md FR-007 vs api.md §5 Functions]
- [ ] CHK017 — Is `POST /api/workflows/:id/execute` endpoint's error behavior documented (what happens when workflow execution fails, times out, hits capability denial)? [Completeness, api.md §6 Workflows execute vs §Errors]
- [ ] CHK018 — Are the Dashboard endpoints (`GET /api/dashboard/stats`) documented with their response schema? [Completeness, spec.md FR-034 (implied) vs api.md §Dashboard]
- [ ] CHK019 — Are the RecommendedGame endpoints (`GET/POST/PUT/DELETE /api/recommended-games`) documented with their full request/response schemas? [Completeness, spec.md FR-034 vs api.md §RecommendedGame]
- [ ] CHK020 — Are the Admin CRUD endpoints (`GET/POST/PUT/DELETE /api/admins`) documented with role requirements and Super protection rules? [Completeness, api.md §Admin vs spec.md FR-023]
- [ ] CHK021 — Are the AdminAuditLog and RuntimeAuditLog list endpoints documented with their filter parameters and response schemas? [Completeness, api.md §AuditLog sections vs data-model.md §AdminAuditLog / §RuntimeAuditLog]
- [ ] CHK022 — Is the `DELETE /api/chat/sessions/:id` endpoint documented with its ownership rules (Super bypass, deleted-creator restriction)? [Completeness, api.md §10 Chat vs SECURITY.md §6 SSE ownership]

## Field Schema Consistency (API vs Data Model)

- [ ] CHK023 — Does the Plugin GET response in api.md §4 include all fields from data-model.md V011 (identifier, version, name, description, manifest, runtime, author, repository_url, s3_key, sha256, size_bytes, category_id, tags, created_at, updated_at, deleted_at), or are some fields intentionally omitted from the API? [Consistency, api.md §4 vs data-model.md §V011]
- [ ] CHK024 — Does the Function POST body in api.md §5 include the `required_capabilities` field added in V029, or is it missing from the API contract? [Completeness, api.md §5 vs data-model.md §V029]
- [ ] CHK025 — Does the Tool POST body in api.md §7 include all V019–V031 fields (source, is_always, category_id, required_capabilities) or are some omitted? [Completeness, api.md §7 vs data-model.md §V014/V019/V020/V026/V029]
- [ ] CHK026 — Does the Skill GET/POST response in api.md §8 include all V021–V031 fields (source, is_always, category_id, required_capabilities) or are some omitted? [Completeness, api.md §8 vs data-model.md §V014/V021/V027/V031]
- [ ] CHK027 — Does the Workflow GET response in api.md §6 include all V032–V036 fields (category_id, input_schema, output_schema, start_description, end_description, required_capabilities, node_type for nodes, node_config for answer nodes)? [Completeness, api.md §6 vs data-model.md §V013/V032/V033/V035/V036]
- [ ] CHK028 — Does the Agent detail response in api.md §9 include `model_preset` and is it consistent with data-model.md V015? [Consistency, api.md §9 vs data-model.md §V015]
- [ ] CHK029 — Is the ChatSession response schema documented with the `admin_phone_snapshot` / `admin_nickname_snapshot` fields from data-model.md V016, or are these intentionally hidden from the API? [Consistency, api.md §10 Chat vs data-model.md §V016]

## Workflow Node Type Documentation

- [ ] CHK030 — Are all 4 node types (function_node, start_node, end_node, generate_answer_node) documented in the api.md §6 graph endpoint response schema, or only function_node? [Completeness, api.md §6 GET graph vs data-model.md §V036]
- [ ] CHK031 — Is the `node_config` JSON field for generate_answer_node documented with its expected sub-fields (system_prompt, model_preset, history_window, variables)? [Completeness, api.md §6 vs data-model.md §V036]
- [ ] CHK032 — Does the PUT graph validation in api.md §6 cover validation rules for non-function_node types (function_id can be NULL for start/end/answer nodes)? [Completeness, api.md §6 PUT graph vs data-model.md §V036]

## SSE Chat Contract Completeness

- [ ] CHK033 — Are ALL SSE event types documented in api.md §10 (token, tool_call, routed, tool_result, done, error), or is tool_result missing from the events list? [Completeness, api.md §10 Chat Events]
- [ ] CHK034 — Is the `tool_result` event format documented (tool_call_id, output), or only the tool_call event? [Completeness, api.md §10 Chat Events]
- [ ] CHK035 — Is the SSE keep-alive ping interval (15 seconds per CHK195) documented in the contract, and is the `: ping` comment format consistent with SSE spec? [Completeness, api.md §10 Chat headers]
- [ ] CHK036 — Are the SSE response headers (Content-Type, Cache-Control, Connection, X-Accel-Buffering per CHK196) documented as mandatory for all SSE endpoints? [Completeness, api.md §10 Chat headers]
- [ ] CHK037 — Is the SSE concurrency limit (CHAT_SSE_MAX_CONCURRENT_PER_ADMIN = 2, error 4291) documented in the chat endpoint section? [Completeness, api.md §10 Chat vs §Errors 4291]

## Capability Contract Consistency

- [ ] CHK038 — Does the capability catalog in host-functions.md §4 (network.http, fs.read/write, s3.read/write, db.query/execute, llm.invoke, secret.get, time.now, log.emit — 8 capabilities, 11 variants) match the capability list in spec.md §Capability table (11 capabilities)? [Consistency, host-functions.md §4 vs spec.md §5 Capability table]
- [ ] CHK039 — Is the `db.execute` / `db.query` capability fully documented in host-functions.md §4.4 (named query only, no free SQL) AND consistent with SECURITY.md TM-4 (SQL injection prevention)? [Consistency, host-functions.md §4.4 vs SECURITY.md TM-4]
- [ ] CHK040 — Are the capability request limits in host-functions.md §5 (30s timeout, 128MB memory, 4MB payload, 8 concurrent HTTP, 100 log/sec, 1000 rows per db.execute) consistent with the environment variable surface in SECURITY.md §8? [Consistency, host-functions.md §5 vs SECURITY.md §8]
- [ ] CHK041 — Is the capability auth chain (envelope parse → lookup → permission check → handler dispatch → audit) documented consistently between host-functions.md §1, SECURITY.md §2, and spec.md FR-003? [Consistency, host-functions.md §1 vs SECURITY.md §2 vs spec.md FR-003]

## Security & RBAC Consistency

- [ ] CHK042 — Are the role requirements for each endpoint group documented in the Capability Coverage Matrix (api.md) AND consistent with spec.md FR-022 (dangerous perms = Super) and FR-023 (main agent edit = Super)? [Consistency, api.md Capability Coverage Matrix vs spec.md FR-022/FR-023]
- [ ] CHK043 — Is the `source = 'builtin'` Tool immutability (V019) documented in the API contract — i.e., does api.md §7 Tools mention that builtin tools cannot be edited? [Completeness, api.md §7 Tools vs data-model.md §V019]
- [ ] CHK044 — Is the `is_always` Tool/Skill implicit authorization behavior documented in the API contract? [Completeness, api.md §7 Tools / §8 Skills vs SECURITY.md §9.2]
- [ ] CHK045 — Are the chat session ownership rules (Super bypass, deleted-creator → Super-only, same-owner) documented in the API contract AND consistent with SECURITY.md §6? [Consistency, api.md §10 Chat vs SECURITY.md §6]
- [ ] CHK046 — Is the SSE chat ownership check applied to the `GET /api/chat/sessions/:id/messages` endpoint as well, or only the POST SSE endpoint? [Completeness, api.md §10 Chat vs SECURITY.md §6]

## Performance Contract Consistency

- [ ] CHK047 — Are the SC targets from perf-evidence.md §1 (SC-001 through SC-010) referenced or documented in the API contract as SLOs? [Completeness, api.md vs perf-evidence.md §1 SC targets]
- [ ] CHK048 — Is the `GET /api/runtime/pool/stats` endpoint response schema documented with all fields (in_use, idle, created_total, cache_misses, wait_count, reset_failures, per_plugin array)? [Completeness, api.md §9b vs perf-evidence.md §5]
- [ ] CHK049 — Does the api.md document the pagination performance expectation (offset beyond total returns empty, not 404) — and is this consistent with perf-evidence.md's FULLTEXT search analysis? [Consistency, api.md §0 pagination vs perf-evidence.md §3]

## Required Capabilities Declaration

- [ ] CHK050 — Is the `required_capabilities` field documented in the API contract for Function/Tool/Workflow/Skill creation endpoints? [Completeness, api.md §5/§6/§7/§8 vs data-model.md §V029-V031]
- [ ] CHK051 — Does the API contract document the validation rule that Tool's required_capabilities must be a superset of its wrapped entity's required_capabilities? [Completeness, api.md §7 Tools vs SECURITY.md §3]

## Host Function ABI Completeness

- [ ] CHK052 — Does the host_call response envelope (`{ ok, data }` for success, `{ ok, code, message }` for failure) match the api.md §0 `host_call response (CHK199)` convention (`ok: false → no data field`, `ok: true → must have data`)? [Consistency, host-functions.md §3 vs api.md §0 CHK199]
- [ ] CHK053 — Is the `secret.get` capability's Super-only requirement documented consistently between host-functions.md §4.6 (Agent permissions must include secret.get, Super-only) and spec.md FR-022? [Consistency, host-functions.md §4.6 vs spec.md FR-022]
- [ ] CHK054 — Is the `log.emit` rate limit (100/sec/Plugin) documented in host-functions.md §5 AND in SECURITY.md's environment variable surface? [Consistency, host-functions.md §5 vs SECURITY.md §8]
- [ ] CHK055 — Does the host-functions.md compatibility section (§7 v1 contract) define what constitutes a breaking change vs non-breaking change with sufficient precision? [Clarity, host-functions.md §7]
- [ ] CHK056 — Is the 4 MB host_call payload limit (host-functions.md §5) consistent with the PLUGIN_MAX_BYTES env (16 MB) in SECURITY.md §8 — or are these two different limits that need clarification? [Clarity, host-functions.md §5 vs SECURITY.md §8]

## Edge Cases & Exception Flows

- [ ] CHK057 — Are zero-state scenarios (no plugins, no agents, no workflows) covered in the API response examples? [Coverage, api.md all sections]
- [ ] CHK058 — Is the behavior documented when a Plugin is deleted while instances are still in the pool? [Coverage, api.md §4 Plugins vs SECURITY.md §1 TM-1]
- [ ] CHK059 — Is the behavior documented when an Agent is deleted while it has active chat sessions? [Coverage, api.md §9 Agents vs data-model.md §V016 (FK SET NULL)]
- [ ] CHK060 — Is the behavior documented when a Capability is removed from the code registry but still exists in the DB and granted to Agents? [Coverage, host-functions.md vs SECURITY.md §1]
- [ ] CHK061 — Is the partial failure behavior documented for Workflow execution (some nodes succeed, then one fails — what happens to prior results)? [Coverage, api.md §6 Workflows execute vs spec.md FR-017]
- [ ] CHK062 — Is the rollback/compensation behavior documented for the Plugin upload pipeline (S3 PUT succeeds but DB INSERT fails)? [Coverage, SECURITY.md §5 plugin upload step 9 vs api.md §4 Plugins]

## Traceability & Naming Consistency

- [ ] CHK063 — Are API endpoint paths consistently named (e.g., `/api/recommended-games` vs `/api/recommended_games` — kebab-case vs snake_case)? [Consistency, api.md all endpoint paths]
- [ ] CHK064 — Are the error codes in api.md §Errors consistent with the error codes referenced in host-functions.md §3 and spec.md? [Consistency, api.md §Errors vs host-functions.md §3 vs spec.md]
- [ ] CHK065 — Does the api.md use consistent terminology for the same concept (e.g., "Plugin" vs "plugin", "Agent" vs "agent", "capability" vs "Capability") across all sections? [Consistency, api.md all sections]
- [ ] CHK066 — Is the `updated_at` optimistic lock convention documented for ALL entities that support it (Plugin/Workflow/Tool/Agent/Skill), or only Skill? [Completeness, api.md §8 Skills vs §4 Plugins / §6 Workflows / §7 Tools / §9 Agents]

## Admin Center & Dashboard API Completeness

- [ ] CHK067 — Are the LoginRecord query endpoints documented with their filter parameters (phone, status, date range) and pagination? [Completeness, api.md vs data-model.md §V002]
- [ ] CHK068 — Is the Dashboard stats response schema documented (plugin/function/workflow/agent/tool/skill/chat counts + recent activity list)? [Completeness, api.md vs spec.md FR-034]
- [ ] CHK069 — Is the Admin password change endpoint documented (separate from general Admin PUT, or combined)? [Completeness, api.md §Admin vs data-model.md §V001]
- [ ] CHK070 — Is the Admin role change protection documented (cannot demote Super, cannot delete Super)? [Completeness, api.md §Admin vs SECURITY.md §4 main agent protections (analogous)]

## Notes

- This checklist focuses on **requirements quality** — checking whether the API contracts are complete, consistent, and unambiguous across all feature documents
- Items are organized by quality dimension: Completeness, Consistency, Clarity, Coverage, and Traceability
- Each item references the specific sections being cross-checked
- Findings should be documented inline with `[x]` for resolved items and notes explaining the resolution
- Priority: Items flagged with [Gap] or [Ambiguity] should be resolved before the API contract is considered stable
