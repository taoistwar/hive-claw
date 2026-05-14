# Contract: HiveClaw OpenResponses v1

**Endpoint**: `POST /v1/responses`
**Owner**: `crates/hiveclaw`
**Caller**: `crates/hivegui` (and any wire-compatible OpenResponses
client)
**Wire reference**: OpenClaw OpenResponses HTTP API —
<https://docs.openclaw.ai/gateway/openresponses-http-api>

This contract pins what HiveClaw v1 MUST accept and return on the wire.
HiveClaw v1 is a placeholder; the response **content** is stub text, but
the response **shape** below is binding. Contract tests in
`crates/hiveclaw/tests/contract_responses_sync.rs` and
`crates/hiveclaw/tests/contract_responses_streaming.rs` assert against
the schemas below.

## Headers

| Header | Direction | Requirement |
|--------|-----------|-------------|
| `Content-Type: application/json` | request | REQUIRED on every request. Bodies with another content type MUST be rejected with `415 Unsupported Media Type`. |
| `Accept: application/json` or `Accept: text/event-stream` | request | OPTIONAL. When present and inconsistent with the request's `stream` field, the server MUST honour `stream` (the field wins over `Accept`). |
| `X-Request-Id` | request | OPTIONAL. If present, the server MUST echo it back on the response and use it as the `request_id` log field. If absent, the server MUST generate a UUID v4 and use that. |
| `X-Request-Id` | response | REQUIRED on every response. |
| `Content-Type: application/json` | response (sync) | REQUIRED when `stream` is false / omitted. |
| `Content-Type: text/event-stream` | response (streaming) | REQUIRED when `stream` is true. |
| `Cache-Control: no-store` | response (streaming) | REQUIRED on streaming responses. |

## Request body

```jsonc
{
  "model": "openclaw:hiveclaw-placeholder-v1",   // REQUIRED. String. Format "<provider>:<agent-id>".
  "input": "<engineer message>",                  // REQUIRED. Either a non-empty string OR a non-empty array of input items (see below).
  "instructions": "<system prompt>",              // OPTIONAL. String. May be null/absent.
  "stream": false,                                // OPTIONAL. Boolean; defaults to false. Selects sync vs SSE response.
  "tools": [],                                    // OPTIONAL. Array. v1 accepts and ignores; an empty array is the documented default.
  "tool_choice": "auto",                          // OPTIONAL. v1 accepts and ignores.
  "max_output_tokens": 2048,                      // OPTIONAL. Positive integer.
  "max_tool_calls": 0                             // OPTIONAL. Non-negative integer.
}
```

### `input` array form

```jsonc
[
  { "role": "user", "content": "<engineer message>" }
]
```

v1 MUST accept either the string form or the array form. For the array
form, v1 MUST extract the concatenation of every `content` field whose
`role == "user"` and feed that to the stub. Other roles (e.g.,
`"developer"`) are accepted but ignored by the placeholder.

### Validation rules (rejected at the axum extractor boundary)

| Rule | Status | Body |
|------|--------|------|
| Body is not valid JSON | `400 Bad Request` | `{ "error": { "type": "invalid_request_error", "message": "request body is not valid JSON" } }` |
| `model` missing or empty | `400 Bad Request` | error.message: `"field 'model' is required"` |
| `model` does not start with `openclaw:` | `400 Bad Request` | error.message: `"field 'model' must be of the form 'openclaw:<agent-id>'"` |
| `input` missing or empty | `400 Bad Request` | error.message: `"field 'input' is required and must be non-empty"` |
| `stream` present but not boolean | `400 Bad Request` | error.message: `"field 'stream' must be a boolean"` |
| `max_output_tokens` ≤ 0 | `400 Bad Request` | error.message: `"field 'max_output_tokens' must be a positive integer"` |
| Body content type ≠ application/json | `415 Unsupported Media Type` | error.message: `"Content-Type must be application/json"` |
| Method ≠ POST | `405 Method Not Allowed` | (default axum body) |
| Path ≠ /v1/responses | `404 Not Found` | (default axum body) |

All `4xx` JSON error bodies share the envelope
`{ "error": { "type": "...", "message": "..." } }`.

## Response — synchronous (`stream: false` or omitted)

**Status**: `200 OK`
**Content-Type**: `application/json`

```jsonc
{
  "id": "resp_01HXYZ...",                        // UUID-derived response id (REQUIRED, unique per call)
  "object": "response",                           // REQUIRED, constant string
  "created": 1715000000,                          // REQUIRED, unix epoch seconds
  "model": "openclaw:hiveclaw-placeholder-v1",    // REQUIRED, echoed from request
  "status": "completed",                          // REQUIRED, "completed" for v1 stub; future: "failed" allowed
  "output": [
    {
      "type": "message",
      "role": "assistant",
      "content": [
        { "type": "output_text", "text": "HiveClaw 占位回复：已收到你的请求。" }
      ]
    }
  ],
  "usage": {
    "input_tokens": 12,                           // REQUIRED integer, stub may approximate
    "output_tokens": 18,                          // REQUIRED integer
    "total_tokens": 30                            // REQUIRED integer = input + output
  }
}
```

**Performance budget**: SC-006 — total response time p95 < 200ms under
representative load. Asserted by `contract_responses_sync.rs`.

## Response — streaming (`stream: true`)

**Status**: `200 OK`
**Content-Type**: `text/event-stream`
**Cache-Control**: `no-store`

Wire format follows the SSE spec: each event is a sequence of `field:
value` lines terminated by a blank line. HiveClaw v1 MUST emit the
following events, in order, on every successful streaming call:

1. `response.created` — emitted within < 200ms of receiving the request
   (SC-006 TTFB budget).
2. One or more `response.output_text.delta` — incremental text fragments
   for the single `output_text` content item.
3. `response.completed` — final response object (same shape as the sync
   response body).
4. The literal terminator `data: [DONE]\n\n`.

### Event 1: `response.created`

```text
event: response.created
data: {"id":"resp_01HXYZ...","object":"response","created":1715000000,"model":"openclaw:hiveclaw-placeholder-v1","status":"in_progress"}

```

### Event 2..N: `response.output_text.delta`

```text
event: response.output_text.delta
data: {"id":"resp_01HXYZ...","delta":"HiveClaw "}

```

`delta` is a UTF-8 string fragment. Concatenating every `delta` in order
MUST equal the final `output[0].content[0].text` of the
`response.completed` event.

### Event N+1: `response.completed`

```text
event: response.completed
data: {"id":"resp_01HXYZ...","object":"response","created":1715000000,"model":"openclaw:hiveclaw-placeholder-v1","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"HiveClaw 占位回复：已收到你的请求。"}]}],"usage":{"input_tokens":12,"output_tokens":18,"total_tokens":30}}

```

### Terminator

```text
data: [DONE]

```

(Note: the `[DONE]` frame has NO `event:` field, only `data: [DONE]`
followed by the blank line. After it, the server closes the response
stream.)

**Performance budget**: SC-006 — time from request received to first
SSE event flushed (the `response.created` frame) MUST be p95 < 200ms.
Total stream duration is not bounded by this contract. Asserted by
`contract_responses_streaming.rs`.

## Error responses

| Condition | Sync mode | Streaming mode |
|-----------|-----------|----------------|
| Validation failure (table above) | `4xx` JSON error envelope | `4xx` JSON error envelope (server refuses to upgrade to SSE before validation) |
| Server-internal failure mid-handler | `500` JSON error envelope | `event: response.failed\ndata: {"error":{"type":"server_error","message":"..."}}\n\n` followed by `data: [DONE]\n\n` |

The streaming `response.failed` event MUST be the last named event
before the `[DONE]` terminator. Clients MUST treat receiving
`response.failed` as a terminal error for that request.

## Logging contract (FR-004 / Constitution Principle VI)

For every `POST /v1/responses` call, HiveClaw MUST emit at least one
structured log record on response completion with these fields:

| Field | Type | Source |
|-------|------|--------|
| `request_id` | string | echoed/generated `X-Request-Id` |
| `operation` | string | constant `"responses.create"` |
| `outcome` | string | one of `"completed"`, `"validation_error"`, `"server_error"`, `"client_disconnect"` |
| `duration_ms` | integer | wall-clock from request received to response closed |
| `stream` | boolean | echoed from the request |
| `status_code` | integer | HTTP status on the response (200 for streaming even when `response.failed` is emitted) |

Logs MUST NOT contain `input`, `instructions`, `output`, or any
fragment of `delta`.

## Forward-compatibility notes (NOT v1 behavior)

- The OpenResponses spec defines additional event types
  (`response.output_item.added`, `response.tool_use.delta`, etc.).
  v1 does NOT emit them; v1 clients (HiveGUI) MUST ignore unknown event
  types so that a later HiveClaw can add them without breaking the
  client.
- The spec defines additional request fields (`temperature`, `top_p`,
  `previous_response_id`, …). v1 accepts and ignores them; a later
  feature will honor them.
