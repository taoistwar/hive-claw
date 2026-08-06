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

# 3. Create a super admin (only once). The password never enters argv,
#    shell history, process listings, or command output.
read -r -s -p "Super-admin password: " HIVEWEB_BOOTSTRAP_PASSWORD
printf '\n'
printf '%s\n' "$HIVEWEB_BOOTSTRAP_PASSWORD" | \
  cargo run -p hiveweb --bin create-super-admin -- \
    --phone 13900000000 --nickname super --password-stdin
unset HIVEWEB_BOOTSTRAP_PASSWORD

# 4. Start the server
cargo run -p hiveweb --bin hiveweb
# → http://localhost:3300
```

HiveWeb handles both SIGINT and SIGTERM. Shutdown stops accepting new
connections, drains accepted requests for at most 30 seconds, stops the
`CompiledPlugin` idle reaper, drops the runtime pool/cache, and closes the DB
pool. A drain timeout cancels remaining requests and continues cleanup.

For secret managers that materialize files, pass
`--password-file /run/secrets/hiveweb-super-admin`; on Unix the file must be a
regular file owned by the current user, have mode 0600 or stricter, and have
exactly one hard link. It is opened once with symlink following disabled, then
validated through that opened descriptor. Plain `--password` is rejected.
Bootstrap passwords use the same policy and bcrypt helper as authenticated
password changes: 6–20 Unicode code points with at least one ASCII letter and
one ASCII digit. The command is INSERT-only. An existing phone fails non-zero and is
never updated; rotate an existing administrator password through the
authenticated password-change flow.

## Background processes

```bash
cargo run --bin audit-retention   # requires secret-managed AUDIT_RETENTION_DATABASE_URL
cargo run --bin chat-retention    # delete chat_sessions > 30 days
```

In production, inject `AUDIT_RETENTION_DATABASE_URL` from the retention
systemd unit or Kubernetes Secret. Do not put it in the HiveWeb service
environment and do not reuse the main application's `DATABASE_URL`. Any
connection or cleanup failure terminates the retention worker after emitting a
static error kind, so the service manager can alert and restart it.
`CHAT_RETENTION_DAYS` and `CHAT_RETENTION_INTERVAL_SEC` must likewise be
strictly positive integers. `chat-retention` exits non-zero with the static
`chat_retention_failed` error kind after a connection or cleanup failure.

For systemd, keep the retention credential in a separate root-owned mode-0600
file and give only the retention unit access:

```ini
[Service]
EnvironmentFile=/etc/hiveweb/audit-retention.env
ExecStart=/opt/hiveweb/bin/audit-retention
Restart=on-failure
```

Use the same separation for a Kubernetes Deployment: mount a dedicated Secret
only into the retention pod and restart on non-zero exit. The binary is a
long-running worker and performs one pass immediately before waiting for the
configured interval.

### Runtime-audit database accounts

Use a migration account for schema changes, the normal HiveWeb account for
application queries, and a third account for retention. Do not grant the
HiveWeb account database-wide DML that would override the table-level boundary;
grant its required business-table privileges explicitly.

```sql
-- HiveWeb application account: append and read audit rows, never mutate them.
GRANT INSERT, SELECT ON hiveweb.runtime_audit_logs TO 'hiveweb_app'@'%';
REVOKE UPDATE, DELETE ON hiveweb.runtime_audit_logs FROM 'hiveweb_app'@'%';

-- Independently deployed retention account: delete expired audit rows only.
CREATE USER 'hiveweb_audit_retention'@'%' IDENTIFIED BY RANDOM PASSWORD;
GRANT DELETE ON hiveweb.runtime_audit_logs TO 'hiveweb_audit_retention'@'%';
```

Verify both identities with `SHOW GRANTS` after provisioning. The main account
must not inherit `UPDATE` or `DELETE` on `runtime_audit_logs` through a broader
database/global grant.

## Developer-only binaries

`api-test` and `seed` are excluded from default/release builds and require the
explicit `dev-tools` feature. `seed-bench` has a separate `bench-tools` gate:

```bash
cargo run -p hiveweb --features dev-tools --bin api-test -- \
  GET /api/quota --body-file /run/secrets/hiveweb-api-request.json \
  --host https://cca.haimacloud.com/
# Or pipe a body from a trusted producer:
jq -nc --arg user_id 448 '{user_id:$user_id}' | \
  cargo run -p hiveweb --features dev-tools --bin api-test -- \
    GET /api/quota --body-stdin --host https://cca.haimacloud.com/
# Pre-set HIVEWEB_DEV_SEED_ADMIN_PHONE to the 11-digit dev-only account.
HIVEWEB_DEV_SEED_ADMIN_PASSWORD_FILE=/run/secrets/hiveweb-dev-seed-admin \
  cargo run -p hiveweb --features dev-tools --bin seed
# DATABASE_URL must already select a disposable hiveweb_bench database.
cargo run -p hiveweb --features bench-tools --bin seed-bench -- \
  100 100 --confirm-destructive hiveweb_bench
```

`api-test` preserves explicit HTTP/HTTPS base URLs but prints only method,
response status, and response body byte count.
It never prints the response body, request body, signing secret, signature
input, or signed URL. `--body-file` uses the same descriptor-based ownership,
mode, symlink, and hard-link checks as administrator password files.
`--body-stdin` rejects an interactive terminal. The legacy positional request
body remains temporarily compatible but is deprecated because argv can appear
in shell history and process listings.

`seed` has no built-in administrator password. It requires
`HIVEWEB_DEV_SEED_ADMIN_PASSWORD_FILE` to pass the descriptor-based private-file
checks described above. It also requires an
11-digit, `1`-prefixed development account in `HIVEWEB_DEV_SEED_ADMIN_PHONE`;
there is no committed default. The password, phone, and nickname are never
printed. Seed password validation and hashing use the same shared policy as
normal administrator password changes.

`seed-bench` deletes non-Super administrator/login fixtures, so it only accepts
a safely parsed MySQL database name ending in `_test` or `_bench` and requires
`--confirm-destructive` to repeat that exact name. Generated accounts receive a
new unreported random password hash on every run; there is no fixed login.

## Binaries

| Bin | Purpose |
|---|---|
| `hiveweb` | Main HTTP server |
| `migrate` | Schema migrations (idempotent; safe to re-run) |
| `create-super-admin` | Bootstrap a role=3 admin |
| `seed` | Dev-only seed data (`--features dev-tools`) |
| `seed-bench` | Destructive disposable-DB fixtures (`--features bench-tools`) |
| `audit-retention` | Cron: prune `runtime_audit_logs` older than `AUDIT_RETENTION_DAYS` |
| `chat-retention` | Cron: prune `chat_sessions` older than `CHAT_RETENTION_DAYS` |
| `api-test` | Dev-only Assistant HTTP client (`--features dev-tools`) |

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
| `default` | no (default false) | **Exactly one** preset must have `default = true`; otherwise startup emits a static safe error and exits non-zero before binding the listener |
| `providers[]` | yes (≥ 1) | First entry = primary; remaining = fallback chain |
| `providers[].kind` | yes | One of: `anthropic`, `azure_openai`, `bedrock`, `github_copilot`, `openai_codex`, `openai_compat` (alias `openai`) |
| `providers[].model` | usually yes | Provider-specific model name (e.g. `gpt-4o-mini`, `claude-opus-4-7`) |
| `providers[].base_url` | no | Override API base URL (e.g. for Azure / proxied endpoints) |
| `providers[].auth` | no (default authenticated) | Only exact `auth = "none"` selects no-auth mode |
| `providers[].api_key_env` | yes by default | Name of a present, non-empty credential env var; forbidden when `auth = "none"` |

The only credential-free form is `auth = "none"` together with an explicit,
syntactically valid `http://` or `https://` `base_url`; omitting
`api_key_env` alone never opts out of authentication.

### Resolution order

When an Agent invokes the LLM:

1. If `agents.model_preset IS NOT NULL` → use that preset
2. Else → use the preset marked `default = true`
3. Within the preset, try `providers[0]` first; on failure walk
   `providers[1..]`. Startup constructs and caches each complete preset chain.
   Runtime call sites retrieve that registered chain through
   `LlmRegistry::build_chain`, which returns the primary provider directly for
   a one-provider preset or a `FallbackProvider` for primary plus fallback
   entries. It neither reconstructs providers per call nor silently downgrades
   a multi-provider preset to primary-only execution.

### Validation

Startup loads the registry before router construction and before the HTTP
listener is bound. It performs the following checks:

- the file at `LLM_PRESETS_PATH` exists and is readable
- TOML parses
- exactly one `default = true`
- the default preset has at least one provider and every provider `kind` can
  be constructed
- every configured `providers[].api_key_env` names a present, non-empty
  environment variable; the name is never interpreted as a literal API key

A missing/unreadable file, malformed TOML, invalid default preset, or invalid
default provider (including a missing referenced credential) causes a static
`llm_registry_load_failed` error and a
non-zero process exit; HiveWeb never starts with an implicit empty registry.
An invalid non-default preset is logged with a static warning and skipped,
because the validated default remains available. Paths, provider details, and
credentials are not included in startup logs.

Successful load logs:

```
LlmRegistry loaded  preset_count=2  default=cheap-fast
```

### Test without API keys

Use a dedicated provider with `auth = "none"` and an explicit valid `http(s)`
`base_url`; do not set `api_key_env` in that provider. Every other provider
must set `api_key_env`, and the referenced environment variable must contain a
non-whitespace value at startup. A missing credential invalidates that preset:
an invalid default aborts startup, while an invalid non-default is statically
warned and skipped.
