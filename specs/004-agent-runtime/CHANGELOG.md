# Changelog — Agent Runtime (004)

> **历史边界（2026-07-23）：** 本文件按发生时间保留原实现事实；其中 US6 admin Chat/SSE、`/api/chat/sessions*`、admin chat 表、`ChatPage` / `ChatStream` 后续已由 `81a84fe` 删除并 superseded。下列 “Added” 条目不是恢复要求；现行普通用户聊天由外部 Assistant API 承担。

All notable changes to this feature branch since fork from `main` (003 admin-center).

## [Unreleased]

### Added — Backend

- **Phase 1 Setup**: extism 1.x + jsonschema 0.17 + path deps on `crates/{agent,providers,skills}` reuse; 15 new env vars catalogued in `.env.example`; `llm_presets.toml.example` + `examples/plugins/README.md` (T001–T005, T149)
- **Phase 2 Foundational**: 11 SQL migrations V008–V018 (capabilities / categories / tags+taggings / plugins / functions / workflows+nodes+edges / tools+skills / agents+ACLs / chat / runtime_audit_logs / seed); 11 sqlx FromRow models; runtime skeleton (capability/pool/invoker/workflow/agent/llm); AppState.runtime_state injection; 12-step startup order documented in plan.md; optimistic_lock helper (T006–T036, T150–T152)
- **US1 Plugin** (T070–T078): upload (WASM magic + sha256 + S3 + DB) / FULLTEXT search + category/tag三维检索 / 三维过滤 / soft-delete with tx + FOR UPDATE + ref-block (FR-007 / SC-009 race window) / optimistic-lock update
- **US2 Function/Tool/Skill** (T081–T087): custom Function CRUD + JSON Schema validation; Tool kind=1/2 mutex with deep-equal schema consistency (invariant #11 / 5002); Skill 64 KB content cap + builtin protection (5008); 3 new routers mounted
- **US7 Category/Tag** (T133–T134): nested categories tree with FK SET NULL on delete; Tag reference-counted delete (4091); polymorphic taggings join
- **US4 Capability Auth** (T095–T105, T162):
  - Dispatcher (`runtime/capability::dispatch`): envelope parse → CapabilityRegistry lookup (4045 unknown) → agent_permissions check (4030 denied) → handler route → audit row
  - 11 capability name constants + static registry
  - Handlers: time.now / log.emit / fs.read / fs.write (sandboxed `/tmp/plugin/`) / network.http (allowlist + SSRF block + 4 MB body cap) / s3.read / s3.write (`plugin-data/` prefix) / secret.get (env allowlist). `db.query` / `db.execute` / `llm.invoke` stubs (returning 5001) until US5+ data plumbing
  - Pool: `HashMap<plugin_id, PluginPool>` with VecDeque<PooledPlugin>; idle/in_use/cache_misses/reset_failures metrics; PoolConfig.from_env reads 5 env vars
  - Invoker: cold-start path with S3 GET + sha256 verify (FR-029 v7 tamper protection) + spawn_blocking Extism compile + host_call host_fn registration via raw form (host_fn! macro hides `plugin` due to hygiene); call_with_host_context bridges per-invocation context
  - GET /api/runtime/pool/stats + POST /api/functions/:id/invoke
- **US3 Workflow DAG** (T108–T109): metadata CRUD; PUT graph with white/gray/black DFS cycle detection (4092 with chain) + mapping required-field check (5005); GET graph returns nodes+edges with node_key references
- **US5 Agent** (T116–T120): Agent CRUD with depth ≤ 10 (5006) + main agent protection (5001 delete, 2001 non-Super edit) + dangerous capability Super-only (2001) + model_preset existence (5007); LlmRegistry::load_from_path with strict exactly-one-default validation; build_primary instantiates `Arc<dyn LLMProvider>` from preset
- **US6 Chat SSE** (T126–T127, T129–T131): session CRUD with admin snapshot columns (FR-027 ownership / 2001 / Super exempt); SSE endpoint with 4 production headers + 15s keep-alive ping + 6 event types (token/tool_call/tool_result/routed/fallback_used/done|error); per-admin concurrency limit (4291 / static SSE_COUNTER + RAII guard)
- **AgentOrchestrator** (T118, T119): multi-hop tool-calling loop with per-hop AgentContext rebuild; OpenAI-style tools schema (route_to_subagent + workspace tools); kind=1 function-wrap dispatch through Invoker; 5-hop guard (AGENT_MAX_HOPS env) + visited[] cycle detection + child-only routing; audit_llm + audit_route events
- **Real LLM streaming** (T128 MVP): chat SSE wired to `providers::build_provider` → `chat_stream(req, on_delta=mpsc push)` → SSE token events; system_prompt + skill markdown joined; full history → ChatRequest messages array
- **Polish**: audit-retention + chat-retention cron binaries (T132 + T145); SECURITY.md doc (T146); perf-evidence.md structural analysis (T142); RuntimePoolCard dashboard widget with cache_misses / reset_failures alert thresholds (T147)
- **/api/capabilities** read-only endpoint (added during validation phase between US7 and US4)
- **17 new business error codes** (4030 / 4045 / 4091..4094 / 4291 / 5001..5009) with HTTP status mapping; CAPABILITY_UNKNOWN renumbered 4040→4045 to avoid 003 NOT_FOUND collision

### Added — Frontend

- 11 new routes wired into `App.tsx`: /plugins /functions /tools /skills /categories /tags /agents /workflows /chat (+ Dashboard runtime card)
- 9 new service modules (plugin / function / tool / skill / category / tag / agent / workflow / chat / capability)
- Pages: PluginPage (upload + 三维检索 + ref-block delete confirm) / FunctionPage / ToolPage / SkillPage (markdown preview drawer) / CategoryPage (tree) / TagPage (reference count) / AgentPage (tree + create with CapabilityPicker + ModelPresetSelect) / WorkflowPage (DAG editor drawer) / ChatPage (session list + SSE stream)
- Components: PluginUploader (sha256 preview + 16 MB cap) / PluginFilters / SchemaEditor (JSON textarea with live parse) / DagEditor (reactflow + client-side cycle highlight) / CycleDetector.ts / AgentTree (depth badges + max-depth warning) / ModelPresetSelect (offline-state warning) / CapabilityPicker (dangerous tags + Super-only checkboxes) / ChatStream (6 event renderers + thinking placeholder) / RuntimePoolCard

### Changed

- `crates/hiveweb/Cargo.toml`: axum +multipart, sqlx +json features; path deps on agent/providers/skills; toml/sha2/reqwest workspace deps; 2 new binary targets (audit-retention + chat-retention)
- `migrate.rs`: 11 V008–V018 entries registered
- `utils/error.rs`: 17 new AppError variants + business code constants + http_status_for_code mappings (4030→403, 4091/4093/4094→409, 4291→429, 5001..5009 various)
- `services/mod.rs` + `api/mod.rs`: 9 new modules mounted behind auth middleware

### Fixed

- V018 seed semicolon in capability description string was splitting migrate.rs statement parser → replaced `;` with `,` (commit ae366a4)
- `POST /api/functions/:id/invoke` builtin 函数调用：移除占位错误 "待 US5 接入 ToolRegistry"，改为直接通过 `builtins::lookup()` + handler 执行（与 orchestrator 同路径）

### Deferred to follow-up

- T079/T080 Builtin function impls + DB upsert 已完成；`agent::ToolRegistry` 装配（让 orchestrator 通过 ToolRegistry 而非直接 handler 调度）仍推迟
- T110 Workflow execute() topological run (deps on invoker; orchestrator currently returns "not yet wired" for kind=2 workspace tools)
- T111 Workflow-wrapped Tool registration
- `db.query` / `db.execute` / `llm.invoke` capability handlers (need named_queries.toml + providers chain reuse)
- FallbackProvider chain wiring (currently primary[0] only)
- T091 SkillMarkdownEditor Monaco + AgentEditor full Monaco system_prompt editor
- Phase 2.5 red-phase tests (T037–T069 + T163) — interleaved into individual US tests instead per project lead decision; integration tests in tests/it_dispatcher.rs + tests/contract_plugin.rs cover the gates
- T137 llm_presets README; T138 examples/plugins/weather full PDK example; T139 a11y full regression CI; T140/T141/T153–T156 criterion microbenchmarks
- T148 final tasks.md sweep + tagging

### Verified live (against running MySQL + Redis + MinIO infra)

- 11 Capability registry exposes correct dangerous flags
- Plugin upload + magic-byte reject + 17MB body reject + 4093 reference-block + soft-delete
- Function/Tool/Skill CRUD with 5002 schema-mismatch + 5008 builtin-protected
- Category tree + Tag 4091 ref-block
- Agent create with 5007 unknown-preset rejection + 5001 main-delete rejection + dangerous capability authorization
- Workflow PUT graph with 4092 cycle detection + 5005 mapping check + happy path persisted
- Dispatcher 4 security gates (4045 unknown / 4030 ungranted / ok=true on granted / 4000 malformed envelope) — 4 integration tests in tests/it_dispatcher.rs PASS
- Plugin invoke: cold-start S3 GET + sha256 verify + Extism compile + host_call host_fn registration; pool stats reflect cache_misses + reset_failures correctly
- SSE chat: 4 critical headers + 15s ping + 6 event types; 2001 ownership + persistence ordering verified
- Real LLM streaming: orchestrator spawns chat_stream task; on_delta → mpsc → SSE token events; LLM error properly surfaces as token+error+done with elapsed_ms; audit row written (event_type=llm_invoke outcome=error)

### Stats

- 12 commits on `004-agent-runtime` branch
- 95/165 tasks marked (Phase 1 + Phase 2 + US1 + US2 + US4 + US5/CRUD + US6/SSE + US7 + significant Phase 10 polish; remaining = deferred items above)
- 0 new TypeScript errors introduced; 16/16 vitest pass
- 1 pre-existing aws_config v2024_03_28 deprecation warning (003 admin-center origin)

Co-Authored-By: Claude Opus 4 (1M context) <noreply@anthropic.com>
