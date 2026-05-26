# Performance Evidence — Agent Runtime

**Status**: Phase 10 Polish artifact (T142 / Principle IV gate)
**Updated**: 2026-05-27

This document records EXPLAIN plans for hot-path queries + structural
analysis for SLO compliance. Live benchmarks (T140 / T141 / T153–T156) are
deferred to a follow-up commit and tracked in `tasks.md` as still open.

## 1. SC targets (from spec.md)

| SC | Target | Component | Coverage in this doc |
|---|---|---|---|
| SC-001 | Plugin upload p95 ≤ 5 s | API + S3 + DB | §3 analysis |
| SC-002 | Plugin list / search p95 ≤ 1 s | DB + FULLTEXT | §3 EXPLAIN |
| SC-003 | Workflow save (50 nodes) p95 ≤ 1 s | service::workflow::put_graph | §4 analysis |
| SC-004 | host_call dispatch p95 ≤ 5 ms | capability::dispatch | §2 path-length |
| SC-005 | Plugin call pool-hit p95 ≤ 50 ms, cold ≤ 300 ms | InstancePool + Extism | §5 analysis |
| SC-006 | Agent routing p95 ≤ 1.5 s (含 1 LLM call) | orchestrator | §6 |
| SC-010 | E2E chat p95 ≤ 8 s | full stack | offline — accepted deviation §7 |

## 2. host_call dispatch path (SC-004 ≤ 5 ms)

Per-call work (no LLM, no plugin call — pure host-side):
1. JSON parse envelope                  — ~10 μs (rusty_json ~1 GB/s)
2. CapabilityRegistry::lookup (HashMap) — O(1), ~100 ns
3. `SELECT capability FROM agent_permissions WHERE agent_id = ?`
   - Index hit on `PRIMARY KEY (agent_id, capability)` → < 1 ms even cold
4. handler dispatch + audit insert (async, fire-and-forget for fast handlers)

**EXPLAIN** for permission lookup:
```sql
EXPLAIN SELECT capability FROM agent_permissions WHERE agent_id = 1;
-- type=ref, key=PRIMARY, rows=N (where N = caps for agent)
-- Extra: Using index (covering index — no table touch)
```

Conclusion: structurally well-within 5 ms. Network DB roundtrip dominates;
co-locate DB + hiveweb to keep p95 < 5 ms.

## 3. Plugin list / FULLTEXT search (SC-002 ≤ 1 s)

`plugins` has:
- `FULLTEXT INDEX ftx_plugins (name, description, identifier)` for search
- `INDEX idx_plugins_category (category_id)` for category filter
- `INDEX idx_plugins_deleted_at (deleted_at)` for soft-delete filter
- `PRIMARY KEY (id)` + `UNIQUE KEY (identifier, version)`

**EXPLAIN** for typical list query:
```sql
EXPLAIN SELECT p.* FROM plugins p
WHERE p.deleted_at IS NULL
  AND MATCH(p.name, p.description, p.identifier) AGAINST ('weather' IN NATURAL LANGUAGE MODE)
ORDER BY p.created_at DESC LIMIT 20 OFFSET 0;
-- type=fulltext, key=ftx_plugins (MySQL prefers fulltext over deleted_at when MATCH present)
-- Extra: Using where; Using filesort (ORDER BY created_at — no covering index)
```

**Optimization opportunity**: add composite index `(deleted_at, created_at DESC)`
to eliminate filesort when search keyword is absent. Deferred — current data
volume (< 10K rows expected per spec scale §) makes filesort negligible.

For 500-row dataset (target T154): full table scan still bounded; expected
p95 well under 1 s with cold cache.

## 4. Workflow PUT graph (SC-003 ≤ 1 s, 50 nodes)

Inside `services::workflow::put_graph`:
- Cycle detection: white/gray/black DFS — O(V+E), pure in-memory
- Mapping validation: O(edges × fields per edge), all in-memory
- DB writes (within transaction):
  - 1× DELETE workflow_edges (CASCADE removes via parent)
  - 1× DELETE workflow_nodes
  - N × INSERT workflow_nodes (50 statements)
  - M × INSERT workflow_edges (typically 1–3× N)

50 INSERTs × ~1 ms each = ~50 ms; cycle DFS < 1 ms. Comfortably under 1 s.

**Optimization opportunity**: batch INSERT (sqlx supports multi-row VALUES)
would drop the 50× roundtrip to 1–2 statements. Deferred — current
performance acceptable.

## 5. Instance Pool acquire + Plugin call (SC-005)

Code path of `runtime::invoker::invoke`:

| Step | Cost (estimated) | Notes |
|---|---|---|
| DB row fetch (plugins by id) | ~1 ms | indexed |
| pool.try_acquire_idle | ~10 μs | Mutex<HashMap> lookup |
| **(cold only)** S3 GET wasm | 10–100 ms | network-bound |
| **(cold only)** sha256 verify | 1–10 ms | 1–16 MB blob |
| **(cold only)** Extism compile | 100–500 ms | wasmtime cranelift JIT |
| call_with_host_context (per host_call inside) | 1–5 ms host + plugin work |
| reset() + release | < 1 ms | |

**Hit path**: ~10 ms total (DB + pool + 1–5 ms plugin work). Target 50 ms p95
achievable.
**Cold path**: 100–500 ms compile dominates. Spec target 300 ms is tight for
larger plugins; first-call slowness mitigated by pool reuse from second
invocation onward.

Live `/api/runtime/pool/stats` already returns:
- `cache_misses` — incremented on cold start (operator alert when growing)
- `reset_failures` — incremented when reset() fails (drops instance — alert)

## 6. Agent routing decision (SC-006 ≤ 1.5 s)

`orchestrator::run_session` per hop:
- DB context fetch (agent + skills + tools + perms + children) — ~10 ms total
  (4 sequential queries — could be parallelized later)
- `llm.build_primary` — config-time provider construction; ~1 ms
- `provider.chat_stream` — **external; dominates**
  - With T156 mock provider returning 800 ms → routing decision = 810 ms
  - Real Anthropic/OpenAI p95 ≈ 1–3 s for short prompts → outside SC-006
    budget (and acknowledged as deviation 4 below)

## 7. End-to-end chat SC-010 (≤ 8 s) — deviation 4 acknowledged

Spec accepts external LLM latency as out-of-control. Tracing in production
should split `host_ms` vs `llm_ms` to separately verify host-side ≤ 200 ms
(internal target).

Current audit rows on `event_type = llm_invoke` record `elapsed_ms` which
includes network + LLM compute. A follow-up improvement: introduce
`host_ms` vs `llm_ms` split (single tracing span pair).

## 8. Index inventory

| Table | Index | Purpose |
|---|---|---|
| plugins | uk_plugins_identifier_version | upload dedup |
| plugins | idx_plugins_category | category filter |
| plugins | idx_plugins_deleted_at | soft-delete filter |
| plugins | ftx_plugins (FULLTEXT) | search |
| functions | identifier UNIQUE | global lookup |
| functions | idx_functions_plugin / idx_functions_category | reverse lookup |
| functions | ftx_functions (FULLTEXT) | search |
| workflow_nodes | uk_workflow_node (workflow_id, node_key) | edit-time uniqueness |
| workflow_edges | idx_workflow_edges_src / dst | edge traversal |
| agents | idx_agents_parent | tree build |
| agent_permissions | PRIMARY KEY (agent_id, capability) | dispatcher hot path |
| chat_sessions | idx_chat_sessions_admin / updated_at DESC | list "my sessions" |
| chat_messages | uk_chat_msg_session_seq | seq monotonicity |
| runtime_audit_logs | idx_ral_capability (cap, outcome) | denial reports |
| runtime_audit_logs | idx_ral_occurred_at DESC | retention scan |

All hot-path queries hit indexes; retention sweeps scan `idx_ral_occurred_at`
in range form (efficient).

## 9. Live benchmark results (placeholder)

T140 / T141 / T153–T156 microbenchmarks not yet wired. Live data will be
appended in a follow-up commit; until then this document captures the
**structural** analysis required by Principle IV gate.

## 10. Action items (post-MVP)

- [ ] Wire criterion benches in `crates/hiveweb/benches/`
- [ ] Add composite index `(deleted_at, created_at DESC)` on plugins
- [ ] Batch INSERT in workflow::put_graph
- [ ] Pool prewarm option (eager compile on Plugin upload, gated by env)
- [ ] Tracing span split: `host_ms` vs `llm_ms` in llm_invoke audit row
