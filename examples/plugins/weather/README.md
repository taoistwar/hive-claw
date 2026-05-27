# Weather Plugin — full Extism PDK example

A complete, compilable Rust example demonstrating the Agent Runtime
Plugin contract:

- `lookup(args_json) -> result_json` export
- `host_call(envelope_json) -> reply_json` to use the host `network.http`
  capability
- proper error propagation via Extism `FnResult`

## Prerequisites

```bash
rustup target add wasm32-unknown-unknown
```

## Build

```bash
cd examples/plugins/weather
cargo build --target wasm32-unknown-unknown --release
# Output: ../../../target/wasm32-unknown-unknown/release/weather_plugin.wasm
```

## Upload via management UI

1. Log in to the admin center (System+ role required)
2. Plugins → Upload Plugin
3. Select `weather_plugin.wasm`
4. Fill metadata:
   - identifier: `weather`
   - name: `Weather Lookup`
   - version: `1.0.0`
   - runtime: `extism` (default)

## Or upload via curl

```bash
TOK=$(curl -s http://localhost:3300/api/auth/login \
  -X POST -H 'Content-Type: application/json' \
  -d '{"phone":"<phone>","password":"<password>"}' \
  | jq -r .data.token)

curl -X POST http://localhost:3300/api/plugins \
  -H "Authorization: Bearer $TOK" \
  -F file=@../../../target/wasm32-unknown-unknown/release/weather_plugin.wasm \
  -F 'meta={"identifier":"weather","name":"Weather","version":"1.0.0","runtime":"extism"}'
```

## Register as a Function

```bash
curl -X POST http://localhost:3300/api/functions \
  -H "Authorization: Bearer $TOK" \
  -H 'Content-Type: application/json' \
  -d '{
    "identifier": "weather.lookup",
    "name": "Look up weather",
    "plugin_id": <id-from-upload>,
    "plugin_export": "lookup",
    "input_schema": {
      "type": "object",
      "properties": {"city": {"type": "string"}},
      "required": ["city"]
    },
    "output_schema": {
      "type": "object",
      "properties": {
        "city": {"type": "string"},
        "temp_c": {"type": "number"},
        "summary": {"type": "string"}
      }
    }
  }'
```

## Wrap as a Tool

```bash
curl -X POST http://localhost:3300/api/tools \
  -H "Authorization: Bearer $TOK" \
  -H 'Content-Type: application/json' \
  -d '{
    "identifier": "weather.lookup",
    "name": "Weather Lookup",
    "description": "Get current weather for a city",
    "kind": 1,
    "function_id": <function-id>,
    "input_schema": { /* same as function */ },
    "output_schema": { /* same as function */ }
  }'
```

## Grant capabilities to an Agent

Before this Plugin can call `network.http`, the Agent invoking it must have
the `network.http` capability granted (dangerous — Super role required).
Also set:

```bash
# In .env on the host:
NETWORK_HTTP_ALLOWLIST=.wttr.in
```

Without an allowlist entry, every call returns
`{"ok":false,"code":4030,"message":"hostname wttr.in 不在 allowlist 内"}`.

## Invoke

```bash
curl -X POST http://localhost:3300/api/functions/<id>/invoke \
  -H "Authorization: Bearer $TOK" \
  -H 'Content-Type: application/json' \
  -d '{"input": {"city": "Beijing"}, "agent_id": 1}'
```

Or via chat:

1. Bind the `weather.lookup` Tool to your Agent
2. Open the Chat page → "Get the weather in Beijing"
3. The LLM will issue a `weather.lookup` tool_call which the orchestrator
   routes through the host runtime.

## Security notes

This example calls `wttr.in` over HTTPS. The host runtime:

- rejects the call if `wttr.in` is not in `NETWORK_HTTP_ALLOWLIST`
- rejects if `wttr.in` resolves to a private/loopback address (SSRF guard)
- caps response body at 4 MB
- disables redirects (to prevent redirect-to-private-address attacks)

See `specs/004-agent-runtime/SECURITY.md` for the full threat model.
