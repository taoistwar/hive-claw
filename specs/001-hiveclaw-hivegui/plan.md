# Implementation Plan: HiveClaw Agent & HiveGUI Client (Initial Scaffold)

**Branch**: `001-hiveclaw-hivegui` | **Date**: 2026-05-14 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/001-hiveclaw-hivegui/spec.md`

## Summary

Stand up two independently-buildable Rust crates inside a single Cargo
workspace: **HiveClaw**, an axum-based HTTP service exposing a
`POST /v1/responses` endpoint wire-compatible with the OpenClaw
OpenResponses spec (supporting both synchronous JSON and SSE streaming via
the `stream` field), and **HiveGUI**, a gpui-based Simplified-Chinese
(`zh-CN`) desktop client that converses with HiveClaw and hosts two
helper-tool sections (Day+1, Hour+1) which ship empty in v1. HiveClaw is a
placeholder that satisfies the wire contract with stub content; HiveGUI is
the real client (sequential single-pending-turn conversation, manual retry,
structured logs to a per-user app-data directory). Both sides run on the
engineer's own machine — no auth, no server tier, no remote deployment.

## Technical Context

**Language/Version**: Rust (stable channel; MSRV pinned via
`rust-toolchain.toml`). No other application language is introduced.
**Primary Dependencies**: `axum` + `tokio` + `tower-http` (HiveClaw HTTP);
`gpui` (HiveGUI desktop UI); `tracing` + `tracing-subscriber` with JSON
formatter (structured logs, both crates); `serde` + `serde_json` (wire
types). Datastores below are wired up at the workspace level but **no
schema or KV namespace is required for v1's placeholder scope**; the
crates pull them in only when a later feature first writes state.
**Storage**: SQLite via `rusqlite` (canonical relational store, deferred
to a later feature); `sled` (canonical embedded KV, deferred to a later
feature). v1 has no persistent state — conversation history lives only
in HiveGUI memory for the session (per spec Assumption).
**Testing**: `cargo test` for unit and integration; `reqwest`-driven
contract tests for HiveClaw's HTTP endpoint; smoke binary launch tests
for HiveGUI (gpui windows are out of cheap headless test scope — see
research.md for the testing strategy compromise).
**Target Platform**: Linux x86_64 primary; macOS arm64 secondary. Windows
is **out of scope for v1** (gpui's Windows backend is still maturing; see
research.md). Both platforms target a single user's local machine.
**Project Type**: desktop-app + co-located service, delivered as a single
Cargo workspace. Two workspace members: `crates/hiveclaw` (binary) and
`crates/hivegui` (binary). No third crate yet (Principle V: defer
`hive-shared` until a second concrete caller justifies it).
**Performance Goals**: HiveClaw `POST /v1/responses` p95 — sync mode:
< 200ms total response time; streaming mode: < 200ms time-to-first-event
(SC-006). HiveGUI startup to home-screen render: < 2s on a recent
laptop. End-to-end user-visible reply: ≤ 3s at p95 against the placeholder
(SC-002).
**Constraints**: < 200ms p95 API budget (constitution IV + SC-006);
single-user local (no auth, no server tier, no network secrets); zh-CN
UI strings (no i18n framework); at most one pending conversation turn at
any time (FR-008a); manual retry only.
**Scale/Scope**: 1 user, 1 HiveGUI window, 1 HiveClaw process per
machine. Conversation length bounded by session memory (no persistence).
v1 ships 4 user stories, 17 functional requirements, 6 success criteria.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

Constitution v1.1.0 (`.specify/memory/constitution.md`). Evaluated:

| Gate | Status | Evidence |
|------|--------|----------|
| I. Code Quality & Maintainability | PASS | `cargo fmt` + `cargo clippy -D warnings` in CI (constitution Tech Stack). No comments-for-the-sake-of-it; public types of the HTTP crate documented. |
| II. Test-First Development (NON-NEGOTIABLE) | PASS | TDD ordering encoded in `/speckit-tasks`: contract tests for `/v1/responses` written and red before HiveClaw handler; HiveGUI conversation flow integration tests red before send/receive code. |
| III. User Experience Consistency | PASS | zh-CN copy for all user-facing strings (FR-012a). Consistent error formats: HiveGUI shows actionable Chinese error + 重试 affordance (Edge Cases). HTTP responses follow OpenResponses spec (one documented convention). |
| IV. Performance & Efficiency | PASS | SC-006 pins the API budget by mode (sync total / stream TTFB, both p95 < 200ms). Placeholder is trivially within budget; contract test asserts it. |
| V. Simplicity & YAGNI | PASS | 2 crates (HiveClaw + HiveGUI), under the ≤3 limit. No shared crate, no premature abstraction. sled / SQLite are deferred to first writing feature — not pulled into v1 if unused. Single Tokio runtime. |
| VI. Observability & Structured Logging | PASS | `tracing` + JSON `tracing-subscriber` for both crates. HiveClaw logs request_id / operation / outcome / duration per request (FR-004). HiveGUI logs to rotating per-user file + stderr (FR-012b). |
| Security: input validation | PASS | axum extractors + `serde` parsing reject malformed `POST /v1/responses` bodies at the boundary (FR-011). HiveGUI sanitises user input before forwarding (FR-011). |
| Security: secrets | PASS | No secrets in code (FR-012). HiveClaw URL is env-injected (`HIVECLAW_URL`); HiveClaw bind address is env-injected (`HIVECLAW_BIND_ADDR`). |
| Security: auth review trigger | N/A | No auth/authz/session/access-control surface in v1 (FR-015). When auth is later added, security review is required. |
| Performance Standards: indexing / N+1 | N/A v1 | No DB queries in v1; gate re-applies when sled / SQLite first land. |
| Workflow: PR + 2-approver core | PASS at process level | Enforced by repo policy, not by code. Plan does not introduce any deviation. |

**Verdict**: All applicable gates PASS. **Complexity Tracking is empty.**

## Project Structure

### Documentation (this feature)

```text
specs/001-hiveclaw-hivegui/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── openresponses-v1.md
├── checklists/
│   └── requirements.md  # Created by /speckit-specify
└── tasks.md             # Phase 2 output (/speckit-tasks; NOT created here)
```

### Source Code (repository root)

```text
# Single Cargo workspace at repo root.
Cargo.toml                # Workspace manifest (workspace.members, shared profile)
rust-toolchain.toml       # Pinned stable toolchain + rustfmt + clippy

crates/
├── hiveclaw/             # Binary crate: axum HTTP service (placeholder agent)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs       # bin entrypoint: parse env, start Tokio runtime, run server
│   │   ├── lib.rs        # crate root: re-export public surface for tests
│   │   ├── config.rs     # env-driven configuration (bind addr, log dir)
│   │   ├── logging.rs    # tracing-subscriber JSON setup
│   │   ├── http/
│   │   │   ├── mod.rs    # axum Router assembly
│   │   │   └── responses.rs  # POST /v1/responses handler (sync + SSE branches)
│   │   ├── openresponses/
│   │   │   ├── mod.rs    # OpenResponses wire types (serde structs)
│   │   │   └── stub.rs   # placeholder reply generator
│   │   └── version.rs    # name + version reporter (FR-002)
│   └── tests/
│       ├── contract_responses_sync.rs       # POST /v1/responses sync contract
│       ├── contract_responses_streaming.rs  # POST /v1/responses SSE contract
│       └── version_smoke.rs                  # FR-002 startup + version
│
└── hivegui/              # Binary crate: gpui desktop client
    ├── Cargo.toml
    ├── src/
    │   ├── main.rs       # bin entrypoint: parse env, start gpui App, open window
    │   ├── lib.rs        # crate root
    │   ├── config.rs     # env-driven configuration (HIVECLAW_URL, log dir)
    │   ├── logging.rs    # tracing-subscriber JSON setup (file + stderr)
    │   ├── client/
    │   │   ├── mod.rs    # reqwest-based OpenResponses client
    │   │   ├── sync.rs   # stream:false call path
    │   │   └── streaming.rs  # stream:true SSE parsing
    │   ├── model/
    │   │   ├── mod.rs
    │   │   ├── conversation.rs  # Conversation + ConversationTurn entities
    │   │   └── tools.rs         # ToolSeries + HelperTool entities (empty lists v1)
    │   ├── ui/
    │   │   ├── mod.rs
    │   │   ├── app.rs           # gpui App, window, root view
    │   │   ├── home.rs          # home screen (会话 / Day+1 / Hour+1 sections)
    │   │   ├── conversation.rs  # conversation surface (single-pending, retry)
    │   │   └── strings_zh.rs    # all user-facing zh-CN strings
    │   └── version.rs
    └── tests/
        ├── client_sync_against_mock.rs       # client hits mock server
        ├── client_streaming_against_mock.rs
        ├── conversation_sequential_send.rs   # FR-008a entity-level test
        └── version_smoke.rs

docs/
└── quickstart.md         # See Phase 1 output (linked from spec / README)
```

**Structure Decision**: Single Cargo workspace at the repo root with two
binary member crates, `crates/hiveclaw` and `crates/hivegui`. This satisfies
Principle V (2 ≤ 3 deployable units; the "shared" third crate is
deliberately not created until a second concrete caller exists). UI strings
are concentrated in `hivegui/src/ui/strings_zh.rs` to keep zh-CN copy
reviewable in one place (per FR-012a). HiveClaw's `openresponses` module
isolates the wire types so the contract is unambiguous and the placeholder
stub can be replaced later without touching the HTTP layer.

## Post-Design Constitution Re-Check (after Phase 1)

Re-evaluated after writing `research.md`, `data-model.md`,
`contracts/openresponses-v1.md`, and `quickstart.md`. No design artifact
introduced a new language, framework, datastore, or deployable unit
beyond what the initial gate cleared:

| Gate | Status | Post-design evidence |
|------|--------|-----------------------|
| I. Code Quality | PASS | No comment-heavy modules planned; public types of `hiveclaw::openresponses` documented by the contract file. |
| II. Test-First | PASS | `contracts/openresponses-v1.md` is the source for the two `contract_responses_*` tests, which `/speckit-tasks` will sequence before the handler. `data-model.md` invariants T1–T4 are the source for `conversation_sequential_send.rs`. |
| III. UX Consistency | PASS | `quickstart.md` shows the zh-CN affordances and the 重试 path verbatim. |
| IV. Performance | PASS | Contract pins SC-006 by mode and binds it to the two contract tests. |
| V. Simplicity & YAGNI | PASS | Still 2 crates. `research.md` §10 explicitly defers sled / SQLite. No shared crate, no config-file layer, no CLI parser. |
| VI. Observability | PASS | Logging contract in `contracts/openresponses-v1.md` and FR-012b in `quickstart.md` §5 are aligned with Principle VI's required fields. |
| Security: input validation | PASS | Validation table in the contract is the boundary; axum extractors enforce it. |
| Security: secrets | PASS | All knobs are env-injected per `research.md` §7; no secrets in repo. |

**Verdict**: No new violations introduced by Phase 1. Complexity
Tracking remains empty.

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

No constitution violations. Table intentionally empty.
