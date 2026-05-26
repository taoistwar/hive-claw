# Security Model — Agent Runtime

**Status**: Phase 10 Polish artifact (T146)
**Audience**: Operators / Reviewers / Plugin authors

## 1. Threat model summary

See `spec.md §Threat Model` for the full TM-1..TM-5 list. This document is the
operational counterpart — *how* the controls are enforced in code.

| Threat | Control | Where enforced |
|---|---|---|
| TM-1 Plugin tampering | sha256 verify before instantiate | `runtime/pool.rs::acquire` (FR-029) |
| TM-2 Capability bypass | Zero-trust deny default | `runtime/capability.rs::dispatch` (FR-003) |
| TM-3 SSRF via network.http | Hostname allowlist + resolve→private-IP rejection | `runtime/capabilities/network_http.rs` |
| TM-4 SQL injection via db.execute | Named-query only — no free SQL | `runtime/capabilities/db.rs` (待 US5+) |
| TM-5 Privilege escalation | Dangerous capability + main agent edit require Super (role=3) | `services/agent.rs::check_dangerous_permissions` + `MAIN_AGENT_IDENTIFIER` guard |

## 2. Capability auth chain

Every Plugin → host_call flows through:

```
host_call(envelope_str) 
  → capability::dispatch
    1. parse envelope JSON           → 4000 BadRequest on malformed
    2. lookup capability name         → 4045 unknown if not registered
    3. load Agent.permissions         → 4030 denied if not granted
    4. handler dispatch (per-cap)     → 4xx/5xx on handler-specific failure
    5. audit row in runtime_audit_logs (success | error | denied | timeout)
```

This applies uniformly whether the Plugin is invoked via:
- Function/Tool from Agent's tool-calling loop (orchestrator)
- POST /api/functions/:id/invoke direct RPC
- Workflow node execution (when T110 lands)

## 3. Dangerous capability grant flow

```
┌─────────────────┐
│ Capability list │  is_dangerous = true:
│  (static)       │     network.http / db.execute / secret.get
└────────┬────────┘
         │
         │ system_admin (role=2) creates Agent with permissions=[...]
         ▼
┌────────────────────────────────────────────────────────┐
│ services/agent.rs::check_dangerous_permissions          │
│   for each cap in permissions:                          │
│     if registry.is_dangerous(cap) && actor_role != 3:   │
│       reject with InsufficientPermission (2001)         │
└─────────────┬──────────────────────────────────────────┘
              │
              ▼
       only Super (role=3) succeeds
              │
              ▼
┌────────────────────────────────────────────────────────┐
│ DB row written to agent_permissions                     │
│ + V006 audit_logs row (003 admin audit — actor + entity)│
└────────────────────────────────────────────────────────┘
```

Frontend `CapabilityPicker` disables dangerous checkboxes for non-Super users
with a Tooltip explanation. Defense-in-depth: both UI + backend reject.

## 4. Main agent protections

`agents.identifier = 'main'` (id=1) is seeded by V018 and is:
- **Not deletable**: `services/agent::delete` rejects with `CannotDeleteMainAgent` (5001)
- **Edit-restricted**: non-Super updates of system_prompt / permissions return 2001
- **Always present**: V018 seeds it with default empty config

`route_to_subagent` only routes to **direct children** of the current agent
(not arbitrary agents in the tree) — enforced in
`runtime/orchestrator.rs::handle_route_tool`.

## 5. Plugin upload pipeline (FR-005)

```
POST /api/plugins (multipart)
  1. Bearer JWT check (role ≥ System)
  2. multipart size limit (axum::DefaultBodyLimit, default 16 MB)
  3. magic-byte check '\0asm'           → 4000 BadRequest on miss
  4. PLUGIN_MAX_BYTES check (env)        → 4000 if exceeded
  5. sha256 over uploaded bytes
  6. uniqueness check (identifier, version) → 4090 Conflict on dup
  7. S3 put (plugins/<identifier>/<version>.wasm)
  8. DB INSERT with sha256, s3_key, size_bytes
  9. On DB failure: spawn fire-and-forget S3 delete to avoid orphan blobs
```

## 6. SSE chat ownership (FR-027)

Three-tier ownership check in `services/chat::check_ownership`:
- `actor_role == Super (3)` → bypass
- `session.admin_id IS NULL` (creator deleted) → only Super
- `session.admin_id != actor_admin_id` → 403 with 2001

Per-admin concurrency limit `CHAT_SSE_MAX_CONCURRENT_PER_ADMIN` (default 2)
enforced via process-local Mutex<HashMap> counter with RAII guard. Exceed →
4291 SseConcurrencyExceeded + HTTP 429.

## 7. Audit retention (FR-022)

`runtime_audit_logs` retained for `AUDIT_RETENTION_DAYS` (default 90).
Background cron: `cargo run --bin audit-retention` (deploy as systemd unit /
kubernetes cronjob). Each pass deletes rows where `occurred_at < NOW() -
INTERVAL N DAY`.

`chat_messages` indirectly retained via parent `chat_sessions` cascade —
`chat-retention` cron deletes sessions older than `CHAT_RETENTION_DAYS`
(default 30).

## 8. Environment variable surface (security-relevant subset)

| Var | Default | Purpose |
|---|---|---|
| `NETWORK_HTTP_ALLOWLIST` | empty | Comma-separated hostnames; **empty = network.http disabled entirely** |
| `SECRET_ALLOWLIST` | empty | Comma-separated env var names readable via secret.get |
| `JWT_SECRET` | (placeholder) | **MUST** be rotated per deployment |
| `PLUGIN_MAX_BYTES` | 16777216 | Cap per-Plugin upload bytes |
| `PLUGIN_CALL_TIMEOUT_MS` | 30000 | Hard kill switch for plugin invocation |
| `PLUGIN_CALL_MAX_MEMORY_MB` | 128 | WASM linear memory cap |
| `CHAT_SSE_MAX_CONCURRENT_PER_ADMIN` | 2 | Per-admin SSE stream cap |

## 9. Known gaps (to address before production)

- [ ] **db.execute / db.query** capability handlers — currently 5001 stub. Once
      named_queries.toml infrastructure lands, **must verify** no free-SQL
      escape path exists.
- [ ] **Fuel-based timeout** on Plugin invocation — currently relying on
      tokio::time + manifest::with_timeout; need Wasmtime fuel limit hookup
      for true CPU-bound runaway protection.
- [ ] **secret.get** is currently env-backed allowlist; a proper KMS / vault
      backend (e.g., HashiCorp Vault, AWS Secrets Manager) recommended for
      production credential rotation.
- [ ] **JWT rotation** — current impl uses static `JWT_SECRET`. Adding key ID
      claim + rotation schedule would close the long-lived-token risk.
- [ ] **request_id forwarding** to dispatcher audit — DispatchCtx accepts
      `request_id` but invoker doesn't yet propagate it from the HTTP layer.

## 10. Reporting security issues

Contact: see top-level `SECURITY.md` (repo root) or
`security@` mailbox per project policy.
