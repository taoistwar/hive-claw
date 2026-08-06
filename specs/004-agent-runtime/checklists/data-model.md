# Data Model Quality Checklist: 004-agent-runtime

> **范围更新（2026-07-23）：** `RecommendedGame` 相关项已废弃；原 admin `chat_sessions` / `chat_messages`、admin snapshot/Super 所有权条目也已由 `81a84fe` 移除并 superseded。相关未勾选项已失效，不是现役缺口。
>
> **迁移编号更新（2026-07-24）：** 下列 V019–V038 与旧 V008–V018
> 映射问题均为历史 checklist 发现，已由 data-model §2 标记
> Historical/Merged。物理 SQL / `MIGRATIONS` 的现行链是 V001–V033，
> V013 空号，V032 runtime audit，V033 timeout default；不得为回答这些
> 历史问题创建或重命名迁移。

**Purpose**: Validate that the data model (data-model.md) is complete, consistent, and unambiguous — cross-referenced against spec.md, contracts/api.md, SECURITY.md, perf-evidence.md, and tasks.md
**Created**: 2026-05-29
**Feature**: [data-model.md](file:///home/developer/agent/hive-claw/specs/004-agent-runtime/data-model.md)
**Depth**: Formal (40+ items)
**Scope**: Cross-doc consistency across all feature documents

## Entity Completeness

- [ ] CHK001 — Are all entities referenced in spec.md FRs represented as tables in data-model.md? [Completeness, spec.md FR-001..FR-034 vs data-model.md §1 entity list]
- [ ] CHK002 — Is the `recommended_games` entity (V022–V025) documented with its full relationship to other entities (or explicitly marked as independent)? [Completeness, data-model.md §2 V022 vs §3 relationship diagram]
- [ ] CHK003 — Does the entity list in data-model.md §1 accurately reflect all 24 tables (Admin through RecommendedGame), or are some missing or duplicated? [Completeness, data-model.md §1 vs §2 V001–V038]
- [ ] CHK004 — Is `agent_permissions` documented as a many-to-many join table (agent_id × capability_name) and is the FK constraint to `capabilities.name` specified? [Completeness, data-model.md §2 V015 vs §4 Invariant 7]

## Field Completeness per Entity

- [ ] CHK005 — Does the `workflows` table include all fields needed for spec.md FR-017 (DAG execution) and FR-033 (required capabilities): identifier, name, description, timeout_ms, required_capabilities, category_id, input_schema, start_description, output_schema, end_description? [Completeness, data-model.md §2 V013/V030/V032/V033/V035 vs spec.md FR-017/FR-033]
- [ ] CHK006 — Does the `workflow_nodes` table include all fields for 4 node types: id, workflow_id, node_key, node_type (ENUM with all 4 variants), function_id (nullable), position (JSON), node_config (JSON)? [Completeness, data-model.md §2 V013/V034/V036 vs spec.md FR-018]
- [ ] CHK007 — Does the `tools` table include all V019–V031 fields: source, is_always, category_id, required_capabilities — and are they consistent with spec.md FR-015? [Completeness, data-model.md §2 V014/V019/V020/V026/V029 vs spec.md FR-015]
- [ ] CHK008 — Does the `skills` table include all V021–V031 fields: is_always, category_id, required_capabilities — and are they consistent with spec.md FR-021? [Completeness, data-model.md §2 V014/V021/V027/V031 vs spec.md FR-021]
- [ ] CHK009 — Does the `runtime_audit_logs` table include all fields needed for SECURITY.md §7 audit retention: occurred_at (for retention scan), capability + outcome (for denial reports), request_id/session_id/agent_id (for tracing)? [Completeness, data-model.md §2 V017 vs SECURITY.md §7]
- [ ] CHK010 — Does the `chat_sessions` table include snapshot fields (admin_phone_snapshot, admin_nickname_snapshot) for audit trace when admin is deleted? [Completeness, data-model.md §2 V016 vs §4 Invariant 12]

## Consistency between DDL and Entity List

- [ ] CHK011 — Does the entity list in data-model.md §1 accurately reflect the key relationships for each table, or are some relationships missing (e.g., `capabilities.category_id`, `skills.category_id`)? [Consistency, data-model.md §1 vs §2 V001–V038]
- [ ] CHK012 — Is the `taggings` entity documented as a polymorphic join table in §1 with its composite key (tag_id + entity_type + entity_id), matching the DDL in V010? [Consistency, data-model.md §1 vs §2 V010]
- [ ] CHK013 — Does the workflow_nodes `node_type` ENUM in DDL (V036: function_node/start_node/end_node/generate_answer_node) match the entity list description in §1? [Consistency, data-model.md §1 vs §2 V036]

## Migration Ordering & Completeness

- [ ] CHK014 — Are all migrations V001–V038 in strict dependency order (e.g., categories before anything that references category_id, agents before agent_permissions)? [Consistency, data-model.md §2 V001–V038 ordering]
- [ ] CHK015 — Is the `capabilities` table (V008) created before `categories` (V009) or after? The V008 DDL references `fk_capabilities_category` which needs categories to exist first — is this an ordering error? [Clarity, data-model.md §2 V008 vs V009]
- [ ] CHK016 — Does V019/V020 (tools source/is_always) properly build on V014 (tools/skills base tables), and are the CHECK constraint changes in V020 compatible with existing data? [Consistency, data-model.md §2 V014 vs V019/V020]
- [ ] CHK017 — Does V026/V027 (tools/skills category_id) avoid creating duplicate FK constraints — i.e., are the V014 DDL `fk_tools_category` / `fk_skills_category` NOT in the base tables, only added via ALTER in V026/V027? [Clarity, data-model.md §2 V014 vs V026/V027]
- [ ] CHK018 — Does V037 (capabilities category_id) conflict with V008 which already has `fk_capabilities_category` — is this a duplicate or is V008 missing it initially? [Conflict, data-model.md §2 V008 vs V037]

## Relationship Diagram Accuracy

- [ ] CHK019 — Does the relationship diagram in §3 accurately represent the self-referencing FK on `agents` (parent_agent_id)? [Consistency, data-model.md §3 vs §2 V015]
- [ ] CHK020 — Is the `Category --< Capability` relationship in §3 accurate given that `capabilities` table has `category_id` (V008/V037)? [Consistency, data-model.md §3 vs §2 V008/V037]
- [ ] CHK021 — Does the diagram show `Agent --*-- Capability (permissions)` which maps to the `agent_permissions` join table — is this clearly represented? [Clarity, data-model.md §3]
- [ ] CHK022 — Is `RuntimeAuditLog` shown as referencing `chat_sessions` (session_id) and `agents` (agent_id) in the diagram? [Completeness, data-model.md §3 vs §2 V017]
- [ ] CHK023 — Does the diagram correctly show that `Tool` can reference EITHER `Function` OR `Workflow` (exclusive OR, not both)? [Clarity, data-model.md §3 vs §2 V014 chk_tools_target]

## Invariant Completeness

- [ ] CHK024 — Are all spec.md FR requirements that imply data integrity rules captured as invariants in §4? [Completeness, spec.md all FRs vs data-model.md §4]
- [ ] CHK025 — Is Invariant 11 (tools kind=1 schema must EQUAL function schema) precisely defined — what does "完全等于" mean for JSON comparison (string equality vs semantic equality)? [Clarity, data-model.md §4 Invariant 11]
- [ ] CHK026 — Is Invariant 12 (chat_sessions with NULL admin_id → Super-only access) documented with its rationale (audit snapshot preservation)? [Clarity, data-model.md §4 Invariant 12]
- [ ] CHK027 — Is Invariant 15 (builtin tools cannot be edited, can only wrap builtin functions) complete — what about deletion of builtin tools? [Completeness, data-model.md §4 Invariant 15 vs SECURITY.md §9.1]
- [ ] CHK028 — Is Invariant 13 (functions do NOT support soft delete — hard delete only) consistent with spec.md FR-007 and the API contract (DELETE /api/functions/:id)? [Consistency, data-model.md §4 Invariant 13 vs spec.md FR-007 vs api.md §5]
- [ ] CHK029 — Is Invariant 14 (skills must be checked against agent_skills before delete) complete — what about skills referenced by other means? [Completeness, data-model.md §4 Invariant 14 vs spec.md FR-021]
- [ ] CHK030 — Are the `is_always` Tool/Skill implicit authorization invariants (16, 17) documented with their security implications — i.e., who can set is_always=1 and audit trail? [Completeness, data-model.md §4 Invariant 16/17 vs SECURITY.md §9.2]
- [ ] CHK031 — Is Invariant 10 (model_preset must match startup-loaded presets) complete — what happens when a preset is removed from llm_presets.toml after Agents reference it? [Coverage, data-model.md §4 Invariant 10]

## Index Coverage

- [ ] CHK032 — Do the indexes documented in §5 (index cheat-sheet) cover all hot-path queries identified in perf-evidence.md §2–§6? [Completeness, data-model.md §5 vs perf-evidence.md §2–§6]
- [ ] CHK033 — Is the `agent_permissions` PRIMARY KEY (agent_id, capability) documented as supporting the capability dispatch hot path (perf-evidence.md §2 SC-004 ≤ 5 ms)? [Consistency, data-model.md §2 V015 vs perf-evidence.md §2]
- [ ] CHK034 — Are FULLTEXT indexes on `plugins` and `functions` documented with their column composition matching the search endpoints in api.md §4/§5? [Consistency, data-model.md §2 V011/V012 vs api.md §4/§5 search params]
- [ ] CHK035 — Is the `idx_taggings_entity` composite index (entity_type, entity_id) documented as supporting the "get all tags for entity" query path used in list endpoints? [Completeness, data-model.md §2 V010 vs §5]

## Soft Delete & Cascade Consistency

- [ ] CHK036 — Is the soft delete strategy consistent across entities: plugins has `deleted_at`, but functions does NOT (Invariant 13) — is this intentional and documented with rationale? [Consistency, data-model.md §2 V011 vs V012 vs §4 Invariant 13]
- [ ] CHK037 — Are all CASCADE delete behaviors documented: taggings CASCADE on tag delete, workflow_nodes/edges CASCADE on workflow delete, agent_tools/skills/permissions CASCADE on agent delete — and are these safe (no unintended data loss)? [Completeness, data-model.md §2 all FK constraints]
- [ ] CHK038 — Are all RESTRICT delete behaviors documented: functions.plugin_id RESTRICT, workflow_nodes.function_id RESTRICT, tools.function_id/workflow_id RESTRICT, agents.parent_agent_id RESTRICT — and do these align with the invariants? [Consistency, data-model.md §2 FK constraints vs §4 invariants]
- [ ] CHK039 — Are all SET NULL delete behaviors documented: login_records.admin_id, admin_audit_logs.actor_admin_id, categories.parent_id, capabilities.category_id, etc. — and do these align with snapshot/audit requirements? [Completeness, data-model.md §2 FK constraints]

## Data Types & Constraints

- [ ] CHK040 — Is the `role` field in `admins` documented with its values: 1=Super, 2=Editor, 3=Viewer — and is this consistent with spec.md's role system (Normal=1 / System=2 / Super=3)? [Conflict, data-model.md §2 V001 vs spec.md clarifications]
- [ ] CHK041 — Is the `status` field in `admins` documented with its values: 1=active, 0=disabled — and is the lockout mechanism (5 failed attempts, 15-min lock) represented in the schema or only application-level? [Completeness, data-model.md §2 V001 vs spec.md auth requirements]
- [ ] CHK042 — Is the `depth` CHECK constraint on agents (depth >= 0 AND depth <= 10) consistent with spec.md's "拒绝 depth > 10" requirement? [Consistency, data-model.md §2 V015 chk_agents_depth vs spec.md FR-023]
- [ ] CHK043 — Is the `functions` CHECK constraint (kind=1 OR kind=2 AND plugin_id IS NOT NULL AND plugin_export IS NOT NULL) complete — does it cover the builtin function case where plugin_id/plugin_export are NULL? [Clarity, data-model.md §2 V012 chk_functions_custom]
- [ ] CHK044 — Is the `tools` CHECK constraint (kind=1 AND workflow_id IS NULL) OR (kind=2 AND function_id IS NULL) complete — does it enforce mutual exclusivity AND prevent both being NULL? [Clarity, data-model.md §2 V014 chk_tools_target]

## JSON Field Documentation

- [ ] CHK045 — Are all JSON-typed fields documented with their expected structure: manifest (extism manifest), input_schema/output_schema (JSON Schema), frontmatter (YAML→JSON mapping), node_config (system_prompt/model_preset/etc.), mapping (edge data mapping), required_capabilities (string array)? [Completeness, data-model.md §2 all JSON columns]
- [ ] CHK046 — Is the `required_capabilities` JSON field documented with its expected format (array of capability name strings) and validation rules (must reference capabilities.name)? [Completeness, data-model.md §2 V029/V030/V031 vs §4 Invariant 7]
- [ ] CHK047 — Is the `workflow_edges.mapping` JSON field documented with its expected format `{"dst.input.foo": "src.output.bar"}` and validation rules (src.output.* must exist in upstream function.output_schema)? [Completeness, data-model.md §2 V013 vs spec.md FR-017]
- [ ] CHK048 — Is the `node_config` JSON field for generate_answer_node documented with its sub-fields (system_prompt, model_preset, history_window, variables) and validation rules? [Completeness, data-model.md §2 V036 vs spec.md FR-018]

## Security & Audit Alignment

- [ ] CHK049 — Does the data model support all threat model mitigations in SECURITY.md TM-1..TM-5? [Completeness, data-model.md vs SECURITY.md §1]
- [ ] CHK050 — Does `runtime_audit_logs` record sufficient fields to support SECURITY.md §6 SSE chat ownership auditing (session_id, agent_id, capability, outcome)? [Completeness, data-model.md §2 V017 vs SECURITY.md §6]
- [ ] CHK051 — Does `admin_audit_logs` record sufficient fields for all admin CRUD operations (actor, action, entity_type, entity_id, old_values, new_values, ip, user_agent)? [Completeness, data-model.md §2 V006 vs spec.md FR-023 audit requirements]
- [ ] CHK052 — Is the `sha256` field on plugins (V011) documented with its purpose (WASM integrity verification before instantiate, SECURITY.md TM-1)? [Completeness, data-model.md §2 V011 vs SECURITY.md §1 TM-1]
- [ ] CHK053 — Does the data model support the configurable runtime audit retention policy (AUDIT_RETENTION_DAYS defaults to 36500 days / 100 years) documented in SECURITY.md §7? [Completeness, data-model.md §V032 vs SECURITY.md §7]

## Edge Cases & Boundary Conditions

- [ ] CHK054 — Is the behavior documented when a category is deleted and multiple entities reference it (SET NULL for FK) — does this leave orphaned entities without categorization? [Coverage, data-model.md §2 all category FK constraints]
- [ ] CHK055 — Is the behavior documented when an admin is deleted and they own chat_sessions / have audit_logs — are the SET NULL behaviors sufficient for audit trail integrity? [Coverage, data-model.md §2 V002/V004/V006/V016]
- [ ] CHK056 — Is the maximum value for `agents.depth` (10) enforced both at the DB level (CHECK constraint) AND application level — and what error is returned when exceeded? [Completeness, data-model.md §2 V015 chk_agents_depth vs spec.md FR-023]
- [ ] CHK057 — Is the behavior documented when `workflow_nodes.function_id` is NULL for non-function_node types — does the FK constraint allow this? [Clarity, data-model.md §2 V036 vs V013 fk_workflow_nodes_function]
- [ ] CHK058 — Are the `content` size limits for skills (64 KB suggestion) documented as a data model constraint or only as an application-level validation? [Clarity, data-model.md §2 V014 skills.content MEDIUMTEXT vs spec.md FR-021]

## Naming & Convention Consistency

- [ ] CHK059 — Is the naming convention consistent across all tables: snake_case for column names, `idx_` prefix for indexes, `fk_` prefix for FKs, `uk_` prefix for unique keys, `chk_` prefix for checks? [Consistency, data-model.md §2 all tables]
- [ ] CHK060 — Are entity names in the relationship diagram (§3) consistent with table names in §2 (e.g., `Admin` entity vs `admins` table)? [Consistency, data-model.md §1 vs §3]
- [ ] CHK061 — Is the `event_type` ENUM in runtime_audit_logs documented with all possible values and their triggering conditions? [Completeness, data-model.md §2 V017 event_type column]

## Cross-Document Traceability

- [ ] CHK062 — Does every FR in spec.md that involves data storage have a corresponding table/field in data-model.md? [Completeness, spec.md FR-001..FR-034 vs data-model.md §2]
- [ ] CHK063 — Does every API endpoint in api.md that returns entity data have its response fields aligned with the corresponding table schema in data-model.md? [Consistency, api.md all sections vs data-model.md §2]
- [ ] CHK064 — Does the capability static registry in data-model.md §6 (11 capabilities) match the host-functions.md §4 capability list (8 capabilities, 11 variants)? [Consistency, data-model.md §6 vs host-functions.md §4]
- [ ] CHK065 — Are the error codes referenced in invariants (5007, 5002, 4093, 4030, 2001) consistent with the error code table in api.md §Errors? [Consistency, data-model.md §4 Invariants vs api.md §Errors]

## 2026-07-24 Migration Remediation

- [x] CHK066 — 物理迁移清单是否与 SQL 文件及
  `migrate.rs::MIGRATIONS` 一致，并明确 V013 空号、V032 runtime audit、
  V033 timeout default？— ✅ data-model §2 已列 V001–V033
- [x] CHK067 — 旧 V019–V038 Agent Runtime 扩展是否明确标记为
  Historical/Merged，且禁止据此新建/重命名 SQL？— ✅ data-model §2.1/2.2

## Notes

- This checklist focuses on **data model requirements quality** — checking whether the schema definitions, invariants, relationships, and constraints are complete, consistent, and unambiguous across all feature documents
- Items are organized by quality dimension: Completeness, Consistency, Clarity, Coverage, and Traceability
- Each item references the specific sections being cross-checked
- Findings should be documented inline with `[x]` for resolved items and notes explaining the resolution
- Key risk areas flagged: V008/V037 capability category FK ordering conflict (CHK018), admin role value mismatch (CHK040), workflow_nodes.function_id NULL constraint (CHK057)
