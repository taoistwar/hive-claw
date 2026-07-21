# hiveweb — Agent Runtime backend

HTTP API service for the Hive-Claw admin center + agent runtime.

## Run

The `hiveweb` server never creates tables or runs migrations during startup.
In production, provision the schema before deploying the service. In development
and test environments, run the migration binary explicitly before every startup
when migrations may have changed.

```bash
# 1. Bring up infra (MySQL + Redis + MinIO)
./scripts/dev-up.sh -d

# 2. Apply schema migrations
cargo run -p hiveweb --bin migrate

# 3. Create a super admin (only once)
cargo run --bin create-super-admin -- \
  --phone 13900000000 --nickname super --password adminpass

# 4. Start the server
cargo run -p hiveweb --bin hiveweb
# → http://localhost:3300
```

## Background processes (deploy as systemd / cron)

```bash
cargo run --bin audit-retention   # delete runtime_audit_logs > 90 days
cargo run --bin chat-retention    # delete chat_sessions > 30 days
```

## Binaries

| Bin | Purpose |
|---|---|
| `hiveweb` | Main HTTP server |
| `migrate` | Schema migrations (idempotent; safe to re-run) |
| `create-super-admin` | Bootstrap a role=3 admin |
| `seed` | Seed test data (003 admin-center scope) |
| `seed-bench` | Generate bench fixtures |
| `audit-retention` | Cron: prune `runtime_audit_logs` older than `AUDIT_RETENTION_DAYS` |
| `chat-retention` | Cron: prune `chat_sessions` older than `CHAT_RETENTION_DAYS` |

## llm_presets.toml

The Agent Runtime selects an LLM provider per Agent via `model_preset`,
which references a named entry in `llm_presets.toml`. Path is configurable
via `LLM_PRESETS_PATH` (default `./llm_presets.toml`).

### File format

```toml
[[preset]]
name = "cheap-fast"
description = "GPT-4o-mini primary, Claude Haiku fallback"
default = true

  [[preset.providers]]
  kind = "openai_compat"
  base_url = "https://api.openai.com/v1"
  model = "gpt-4o-mini"
  api_key_env = "OPENAI_API_KEY"

  [[preset.providers]]
  kind = "anthropic"
  model = "claude-haiku-4-5-20251001"
  api_key_env = "ANTHROPIC_API_KEY"

[[preset]]
name = "code-expert"
description = "Claude Opus primary, GPT-4o fallback"
default = false

  [[preset.providers]]
  kind = "anthropic"
  model = "claude-opus-4-7"
  api_key_env = "ANTHROPIC_API_KEY"

  [[preset.providers]]
  kind = "openai_compat"
  base_url = "https://api.openai.com/v1"
  model = "gpt-4o"
  api_key_env = "OPENAI_API_KEY"
```

### Field reference

| Field | Required | Notes |
|---|---|---|
| `name` | yes | Identifier used by `agents.model_preset` |
| `description` | yes | Human-readable description shown in the ModelPresetSelect dropdown |
| `default` | no (default false) | **Exactly one** preset must have `default = true`; startup will panic otherwise |
| `providers[]` | yes (≥ 1) | First entry = primary; remaining = fallback chain |
| `providers[].kind` | yes | One of: `anthropic`, `azure_openai`, `bedrock`, `github_copilot`, `openai_codex`, `openai_compat` (alias `openai`) |
| `providers[].model` | usually yes | Provider-specific model name (e.g. `gpt-4o-mini`, `claude-opus-4-7`) |
| `providers[].base_url` | no | Override API base URL (e.g. for Azure / proxied endpoints) |
| `providers[].api_key_env` | yes (for cloud) | Name of the env var that holds the API key |

### Resolution order

When an Agent invokes the LLM:

1. If `agents.model_preset IS NOT NULL` → use that preset
2. Else → use the preset marked `default = true`
3. Within the preset, try `providers[0]` first; on failure walk
   `providers[1..]` (FallbackProvider chain — **wiring is in-progress**:
   the current commit only invokes `providers[0]`; chain failover is
   tracked in the CHANGELOG as a deferred item)

### Validation

Startup performs the following checks; **any failure → panic** (per
`plan.md §Startup Initialization Order`):

- file at `LLM_PRESETS_PATH` exists OR `LlmRegistry` is empty (warn only —
  Agents cannot use `model_preset` but service starts)
- TOML parses
- exactly one `default = true`
- each `kind` is recognized

Successful load logs:

```
LlmRegistry loaded  preset_count=2  default=Some("cheap-fast")
```

### Test without API keys

If no `*_API_KEY` env var is set, provider construction succeeds but the
first `chat_stream` call returns a network/auth error. The chat SSE flow
surfaces this as an `error` event followed by a `done` event — the wire
format is verifiable without burning real LLM quota.
