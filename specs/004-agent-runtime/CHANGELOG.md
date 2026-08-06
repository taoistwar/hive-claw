# Changelog — Agent Runtime (004)

> **历史边界（2026-07-23）：** 本文件按发生时间保留原实现事实；其中 US6 admin Chat/SSE、`/api/chat/sessions*`、admin chat 表、`ChatPage` / `ChatStream` 后续已由 `81a84fe` 删除并 superseded。下列 “Added” 条目不是恢复要求；现行普通用户聊天由外部 Assistant API 承担。

All notable changes to this feature branch since fork from `main` (003 admin-center).

## [Unreleased]

### Added — Runtime and CI convergence (2026-07-24)

- Added an explicit `RuntimeExecutionContext` across Assistant, Orchestrator,
  Hook, Workflow, Invoker and Capability paths. Normal requests use bounded
  best-effort DB audit while entering Hook permanently narrows the nested chain
  to tracing-only without losing request/session correlation.
- Added bounded audit-writer health metrics to pool stats and the Dashboard:
  enqueued, persisted, tracing-only, queue-full drops, writer-closed drops,
  no-writer drops and persistence failures.
- Added process-local Capability limits: at most eight concurrent
  `network.http` calls per `(plugin_id, session_id)` and a per-Plugin
  100-events/s `log.emit` token bucket. Rejections use the new 4292 business
  code; the superseded admin SSE code 4291 is not reused.
- Workflow metadata and Graph conversion now preserve start/end descriptions;
  the admin DAG editor can symmetrically view, edit and serialize both
  descriptions. Schema-less Workflow execution returns the sole terminal
  output directly or a deterministic node-keyed object for multiple terminals.
- Hook Function/Workflow actions merge configured `args`, protect the
  runtime-owned HookContext and `_agent_context` snapshot from spoofing, and keep
  `_agent_context_updates` internal to controlled in-memory state updates.
- LLM preset loading is now a required pre-Router startup step: missing or
  invalid default configuration exits non-zero with a static error instead of
  constructing an empty registry; invalid non-default presets are safely
  skipped.
- CI, workspace metadata and `rust-toolchain.toml` now pin exact Rust 1.97.1
  and verify both rustc and cargo releases before locked builds.

### Security — Operational tools (2026-07-24)

- `audit-retention` now requires its dedicated least-privilege database URL and
  fails closed with static error categories on connection or cleanup failure.
- Development-only API/seed binaries are feature-gated; diagnostic output no
  longer prints response bodies or signed URLs, and bootstrap passwords are
  accepted only through stdin or permission-restricted files.
- Secret/request-body files use a single `O_NOFOLLOW` open followed by
  descriptor ownership, mode, regular-file and hard-link checks.
  `seed-bench` has a separate feature gate plus parsed disposable-DB and exact
  destructive confirmation guards; `chat-retention` fails closed on invalid
  intervals and runtime errors.
- Hook Webhooks now share the capability HTTP transport policy: HTTPS-only,
  complete DNS-answer public-IP validation, DNS pinning, no proxy, no redirect
  following, and strict header parsing on create/update, first attempt and
  retries. Client request IDs are accepted only as canonical non-nil UUIDs.
- Invoker and Capability early failures enter exactly-once sanitized audit
  guards, and Extism explicitly disables WASI while retaining the registered
  capability bridge.
- Password validation now counts 6–20 Unicode code points and still requires
  an ASCII letter and digit. DNS failures are typed as `Policy` versus
  `ResolveUnavailable`; Hook retries only `ResolveUnavailable`, reqwest
  connection errors and attempt timeouts, while policy/header/client-build,
  other request and HTTP non-2xx failures are terminal.
- The Plugin pool enforces per-Plugin/global limits, FIFO timeout-to-5009,
  `wait_count`, cancellation-safe permits and an idle-reaper lifecycle. It
  caches only `CompiledPlugin`; every call creates fresh Store/Instance state.
- DAG v2 migration writes v3 only after a successful Graph refresh and removes
  v2 only after the v3 write succeeds; refresh failure retains v2 and writes no
  v3. Workflow scalar outputs stay scalar, End renders only public outputs, and
  diagnostic `node_results` strip `_agent_context_updates`.
- LLM providers require `api_key_env` by default. Credential-free mode requires
  `auth="none"` plus an explicit valid HTTP(S) base URL and forbids a key env.

### Implemented — Approved 25A–29A follow-up (2026-07-24)

- Admin performance reproduction now uses the `bench-tools` gate, an exactly
  confirmed parsed disposable `_test`/`_bench` database, stdin-only bootstrap
  credentials, and a private request-body file instead of a fixed example
  password or argv secrets (T254 complete).
- LLM registry failures emit only static safe errors and exit non-zero before
  listening; successful logs render `default=cheap-fast`, not an `Option`
  debug value (T256 complete). Multi-provider runtime call sites must obtain
  the startup-cached complete chain through `LlmRegistry::build_chain` instead
  of primary-only execution; this does not introduce per-call provider
  construction (T255/T259 complete; wiring and lifecycle tests pass).
- Pool capacity is `in_use + reserved`; idle compiled cache consumes no permit.
  Only successful real compilation increments `cache_misses`, and only a
  successfully created fresh Store/Instance increments `created_total`
  (T257 complete; 13 focused pool tests pass).
- SIGINT/SIGTERM stop accept, drain for at most 30 seconds, stop and join the
  idle reaper, drop the pool/cache, then close the DB pool. Deadline expiry
  cancels active handler, response-body and request-child futures before
  cleanup (T258 complete; six local-server shutdown tests pass).

### Implemented — Approved 30A–34A LLM remediation (2026-07-24)

- T259 implements startup construction and `Arc` caching of complete provider
  chains, removes `build_primary`, and resolves fallback identity by preset
  name + provider ordinal rather than model text. Its Green lifecycle tests
  complete T255.
- T260 implements preset generation settings as defaults only. A legal explicit
  `max_tokens` or `temperature` survives every provider switch;
  `llm.invoke max_tokens=0` is instead rejected as 4001 before DB/provider work.
- T261 implements structured `actual_model` / `fallback_used` / `reason`
  results and matching sanitized tracing/audit. Provider switches use
  `llm_fallback`; application-generated local text uses
  `llm_local_fallback` and is not a provider fallback.
- T262 enforces 25 seconds per provider, 45 seconds for an ordinary chain,
  and 25 seconds for the complete `llm.invoke` chain inside the 30-second
  Plugin budget.
- T263 keeps `build_chain(None)` as the registry-level default selection and
  makes every explicit unknown preset fail closed. For `llm.invoke`
  specifically, only an existing Agent DB row whose `model_preset` is NULL
  selects default; a missing row fails before building or contacting a provider.
- T255 and T259–T263 completed strict Red → Green across registry/provider,
  Workflow, Capability, builtin and runtime-call-site focused tests.

### Fixed — Contract and migration truth (2026-07-24)

- Workflow metadata create/update now reject `timeout_ms` outside
  `1000..=330000` before DB work; POST still defaults to 33000 ms and preserves
  category, required capabilities and tag associations. The HTTP contract now
  lists the real POST/PUT fields.
- Runtime audit RangePicker filters now serialize with `toISOString()` as
  RFC3339 UTC; a UTC+8 regression test proves the instant is not shifted.
- Migration documentation now follows the physical SQL and
  `migrate.rs::MIGRATIONS`: V001–V033, V013 reserved, V032 runtime audit, V033
  Workflow timeout default. The old “V019–V038 Agent Runtime extensions” are
  marked Historical/Merged and must not be recreated or renamed.
- Capability unknown code 4045 now maps to HTTP 400 instead of an internal
  server error.
- The `log.emit` token bucket uses a monotonic clock and remains stable when
  observations arrive out of order.
- The Workflow Graph response example now includes virtual start/end nodes and
  uses the actual `src_node_key` / `dst_node_key` edge fields.

### Added — Runtime audit restore (2026-07-23)

- V032 restores sanitized non-Hook `runtime_audit_logs` persistence behind a
  bounded, non-blocking single worker. Safe tracing remains primary, Hook stays
  tracing-only, read access is Super-only GET API/UI, and configurable
  retention defaults to 36500 days (100 years).

### Added — Backend

- **Phase 1 Setup**: extism 1.x + jsonschema 0.17 + path deps on `crates/{agent,providers,skills}` reuse; 15 new env vars catalogued in `.env.example`; `llm_presets.toml.example` + `examples/plugins/README.md` (T001–T005, T149)
- **Phase 2 Foundational（历史规划编号，现已 superseded）**: 当时文档写作
  “V008–V018”；当前物理映射是 Agent Runtime 基础表 V005–V012、seed
  V014、runtime audit V032。其余 runtime skeleton / model / startup /
  optimistic-lock 实现记录保持有效（T006–T036, T150–T152）
- **US1 Plugin** (T070–T078): upload (WASM magic + sha256 + S3 + DB) / FULLTEXT search + category/tag三维检索 / 三维过滤 / soft-delete with tx + FOR UPDATE + ref-block (FR-007 / SC-009 race window) / optimistic-lock update
- **US2 Function/Tool/Skill** (T081–T087): custom Function CRUD + JSON Schema validation; Tool kind=1/2 mutex with deep-equal schema consistency (invariant #11 / 5002); Skill 64 KB content cap + builtin protection (5008); 3 new routers mounted
- **US7 Category/Tag** (T133–T134): nested categories tree with FK SET NULL on delete; Tag reference-counted delete (4091); polymorphic taggings join
- **US4 Capability Auth** (T095–T105, T162):
  - Dispatcher (`runtime/capability::dispatch`): envelope parse → CapabilityRegistry lookup (4045 unknown) → agent_permissions check (4030 denied) → handler route → audit row
  - 11 capability name constants + static registry
  - Handlers: time.now / log.emit / fs.read / fs.write (sandboxed `/tmp/plugin/`) / network.http (allowlist + SSRF block + 4 MB body cap) / s3.read / s3.write (`plugin-data/` prefix) / secret.get (env allowlist). `db.query` / `db.execute` / `llm.invoke` stubs (returning 5001) until US5+ data plumbing
  - Pool（历史实现，已由 Unreleased 的 T250 设计 supersede）: `HashMap<plugin_id, PluginPool>` with VecDeque<PooledPlugin>; idle/in_use/cache_misses/reset_failures metrics; PoolConfig.from_env reads 5 env vars
  - Invoker: cold-start path with S3 GET + sha256 verify (FR-029 v7 tamper protection) + spawn_blocking Extism compile + host_call host_fn registration via raw form (host_fn! macro hides `plugin` due to hygiene); call_with_host_context bridges per-invocation context
  - GET /api/runtime/pool/stats + POST /api/functions/:id/invoke
- **US3 Workflow DAG** (T108–T109): metadata CRUD; PUT graph with white/gray/black DFS cycle detection (4092 with chain) + mapping required-field check (5005); GET graph returns nodes+edges with node_key references
- **US5 Agent (historical initial wiring)** (T116–T120): Agent CRUD with depth ≤ 10 (5006) + main agent protection (5001 delete, 2001 non-Super edit) + dangerous capability Super-only (2001) + model_preset existence (5007); LlmRegistry::load_from_path with strict exactly-one-default validation; initial `build_primary` behavior is superseded by the T255 `build_chain` contract
- **US6 Chat SSE** (T126–T127, T129–T131): session CRUD with admin snapshot columns (FR-027 ownership / 2001 / Super exempt); SSE endpoint with 4 production headers + 15s keep-alive ping + 6 event types (token/tool_call/tool_result/routed/fallback_used/done|error); per-admin concurrency limit (4291 / static SSE_COUNTER + RAII guard)
- **AgentOrchestrator** (T118, T119): multi-hop tool-calling loop with per-hop AgentContext rebuild; OpenAI-style tools schema (route_to_subagent + workspace tools); kind=1 function-wrap dispatch through Invoker; 5-hop guard (AGENT_MAX_HOPS env) + visited[] cycle detection + child-only routing; audit_llm + audit_route events
- **Real LLM streaming** (T128 MVP): chat SSE wired to `providers::build_provider` → `chat_stream(req, on_delta=mpsc push)` → SSE token events; system_prompt + skill markdown joined; full history → ChatRequest messages array
- **Polish（历史实现）**: audit-retention + chat-retention cron binaries (T132 + T145); SECURITY.md doc (T146); perf-evidence.md structural analysis (T142); RuntimePoolCard dashboard widget with cache_misses / reset_failures alert thresholds (T147; reset alert 已由 Unreleased T250 的 fresh Store/Instance 设计 supersede)
- **/api/capabilities** read-only endpoint (added during validation phase between US7 and US4)
- **17 new business error codes** (4030 / 4045 / 4091..4094 / 4291 / 5001..5009) with HTTP status mapping; CAPABILITY_UNKNOWN renumbered 4040→4045 to avoid 003 NOT_FOUND collision

### Added — Frontend

- 11 new routes wired into `App.tsx`: /plugins /functions /tools /skills /categories /tags /agents /workflows /chat (+ Dashboard runtime card)
- 9 new service modules (plugin / function / tool / skill / category / tag / agent / workflow / chat / capability)
- Pages: PluginPage (upload + 三维检索 + ref-block delete confirm) / FunctionPage / ToolPage / SkillPage (markdown preview drawer) / CategoryPage (tree) / TagPage (reference count) / AgentPage (tree + create with CapabilityPicker + ModelPresetSelect) / WorkflowPage (DAG editor drawer) / ChatPage (session list + SSE stream)
- Components: PluginUploader (sha256 preview + 16 MB cap) / PluginFilters / SchemaEditor (JSON textarea with live parse) / DagEditor (reactflow + client-side cycle highlight) / CycleDetector.ts / AgentTree (depth badges + max-depth warning) / ModelPresetSelect (offline-state warning) / CapabilityPicker (dangerous tags + Super-only checkboxes) / ChatStream (6 event renderers + thinking placeholder) / RuntimePoolCard

### Changed

- `crates/hiveweb/Cargo.toml`: axum +multipart, sqlx +json features; path deps on agent/providers/skills; toml/sha2/reqwest workspace deps; 2 new binary targets (audit-retention + chat-retention)
- `migrate.rs`（当前真值）: 注册 V001–V033，跳过保留空号 V013，最后一项
  为 V033
- `utils/error.rs`: 17 new AppError variants + business code constants + http_status_for_code mappings (4030→403, 4091/4093/4094→409, 4291→429, 5001..5009 various)
- `services/mod.rs` + `api/mod.rs`: 9 new modules mounted behind auth middleware

### Fixed

- V014 seed semicolon in capability description string was splitting migrate.rs statement parser → replaced `;` with `,` (commit ae366a4)
- `POST /api/functions/:id/invoke` builtin 函数调用：移除占位错误 "待 US5 接入 ToolRegistry"，改为直接通过 `builtins::lookup()` + handler 执行（与 orchestrator 同路径）
- S3 自定义 endpoint 默认启用 path-style 桶寻址，并允许通过 `AWS_S3_FORCE_PATH_STYLE` 调整；避免 Rustfs / MinIO bucket 被拼成不可解析的虚拟主机名。

### Deferred to follow-up

- T079/T080 Builtin function impls + DB upsert 已完成；`agent::ToolRegistry` 装配（让 orchestrator 通过 ToolRegistry 而非直接 handler 调度）仍推迟
- T110 Workflow execute() topological run (deps on invoker; orchestrator currently returns "not yet wired" for kind=2 workspace tools)
- T111 Workflow-wrapped Tool registration
- **Resolved in Unreleased T260–T263:** `llm.invoke` now uses the current
  Agent preset, bounded cached chain and typed validation/failure behavior
- **Resolved in Unreleased T255/T259:** FallbackProvider chain wiring now
  forbids primary-only runtime and reuses startup-cached complete chains
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
- Plugin invoke（历史实现）: cold-start S3 GET + sha256 verify + Extism compile + host_call host_fn registration; pool stats reflect cache_misses + reset_failures correctly（reset 语义已由 Unreleased T250 supersede）
- SSE chat: 4 critical headers + 15s ping + 6 event types; 2001 ownership + persistence ordering verified
- Real LLM streaming: orchestrator spawns chat_stream task; on_delta → mpsc → SSE token events; LLM error properly surfaces as token+error+done with elapsed_ms; audit row written (event_type=llm_invoke outcome=error)

### Stats

- 12 commits on `004-agent-runtime` branch
- 95/165 tasks marked (Phase 1 + Phase 2 + US1 + US2 + US4 + US5/CRUD + US6/SSE + US7 + significant Phase 10 polish; remaining = deferred items above)
- 0 new TypeScript errors introduced; 16/16 vitest pass
- 1 pre-existing aws_config v2024_03_28 deprecation warning (003 admin-center origin)

Co-Authored-By: Claude Opus 4 (1M context) <noreply@anthropic.com>
