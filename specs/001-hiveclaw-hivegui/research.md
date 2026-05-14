# Phase 0 Research: HiveClaw Agent & HiveGUI Client (Initial Scaffold)

**Branch**: `001-hiveclaw-hivegui` | **Date**: 2026-05-14 |
**Plan**: [plan.md](./plan.md) | **Spec**: [spec.md](./spec.md)

## Purpose

The Technical Context in `plan.md` is already constrained by Constitution
v1.1.0's Technology Stack section (Rust + gpui + axum + sled + SQLite +
Tokio). This research file does not re-open those choices. It records the
concrete, plan-level decisions that follow from that stack and from the
spec's clarifications: which crates inside the canonical ecosystem the v1
scaffold pulls in, and the few testing / packaging compromises that have to
be made explicit so the `/speckit-tasks` phase can lean on them.

Each entry below uses the required format: **Decision / Rationale /
Alternatives considered.**

## 1. HiveClaw HTTP server crates

**Decision**: `axum` 0.7.x on the `tokio` 1.x multi-thread runtime, with
`tower-http` for tracing/request-id middleware and `serde` + `serde_json`
for the OpenResponses wire types.

**Rationale**: Constitution Tech Stack mandates axum + Tokio. axum 0.7 is
the current stable line, integrates SSE via `axum::response::sse::Sse`
without a third-party crate, and exposes the extractor model that lets
malformed `POST /v1/responses` bodies be rejected at the boundary per
constitution Security (input validation). `tower-http` is the project-blessed
middleware home and brings `TraceLayer` + `SetRequestIdLayer` so FR-004's
"request_id / operation / outcome / duration" log fields are populated by
the framework, not by hand-rolled handler code.

**Alternatives considered**:
- *actix-web*: equally capable, but introduces a parallel HTTP framework
  the constitution forbids in v1.
- *Roll our own SSE writer over hyper*: smaller surface, but reimplements
  what axum already ships; violates Principle V ("use frameworks
  directly").

## 2. HiveClaw SSE format & terminator

**Decision**: Emit OpenResponses streaming events as SSE frames where
`event:` carries the event name (`response.output_text.delta`,
`response.completed`, etc.) and `data:` carries the JSON payload. The
stream MUST terminate with a single `data: [DONE]\n\n` sentinel and then
close, as documented in the OpenClaw OpenResponses spec.

**Rationale**: The spec's clarification round 2 explicitly anchors HiveClaw
to OpenClaw's wire format. HiveGUI's streaming client parses this format;
adopting it verbatim means HiveGUI needs no agent-specific shim. axum's
`Sse` response type writes the `event:` / `data:` framing for us — we only
have to feed it a `Stream` of `sse::Event` values.

**Alternatives considered**:
- *JSON-NDJSON over a plain chunked response*: simpler on the wire but
  breaks compatibility with the spec the user pinned.
- *WebSocket*: bidirectional capability is not needed; SSE is the
  documented OpenResponses transport.

## 3. HiveClaw placeholder reply generation

**Decision**: `hiveclaw::openresponses::stub` returns a fixed, deterministic
reply: a short Simplified-Chinese string identifying the placeholder
(e.g. `"HiveClaw 占位回复：已收到你的请求。"`) plus echoed metadata
(request id, received input length). In streaming mode the stub emits 2–4
`response.output_text.delta` events spaced by a small `tokio::time::sleep`
(< 50ms total) so HiveGUI's incremental-render path is genuinely exercised,
followed by `response.completed` and `[DONE]`.

**Rationale**: A non-trivial-but-bounded stub gives HiveGUI's streaming
renderer a real signal to react to while keeping the synchronous-mode p95
trivially under 200ms (SC-006). Determinism keeps the contract test stable.

**Alternatives considered**:
- *Pure echo of the user input*: tempting but risks leaking unsanitised
  input back into the SSE stream during early development.
- *Random-length filler*: makes contract tests flaky.

## 4. HiveGUI desktop framework target matrix

**Decision**: Linux x86_64 is the primary target; macOS arm64 is a
secondary target built from the same source. Windows is **out of scope for
v1**. The `hivegui` crate's `Cargo.toml` declares no Windows-specific
features and `main.rs` does not attempt Windows-only system calls.

**Rationale**: gpui is developed primarily against Linux and macOS; its
Windows backend is still maturing. The spec's single-user / single-machine
posture (FR-015) does not require Windows, and Constitution Principle V
discourages preemptive cross-platform work without a concrete user.

**Alternatives considered**:
- *Target Windows now*: would add a CI matrix dimension and likely
  blocking issues with gpui's Win32 backend before v1 ships.
- *Linux-only*: cheaper, but the project lead's machine is macOS-friendly
  and the cost of keeping macOS green is low while gpui already targets
  it as a first-class platform.

## 5. HiveGUI ↔ HiveClaw client crate choice

**Decision**: `reqwest` with the `rustls-tls` + `stream` features for the
HTTP client; SSE parsing handled by `eventsource-stream` (a thin
`futures::Stream` adapter over a reqwest byte stream) so we do not have
to hand-roll the SSE frame state machine.

**Rationale**: reqwest is the de-facto Rust HTTP client and rustls keeps
the dependency tree free of system OpenSSL on Linux/macOS. `eventsource-
stream` is a tiny, well-scoped crate; its alternative is reimplementing
the SSE parser, which is exactly the kind of premature abstraction
Principle V prohibits.

**Alternatives considered**:
- *hyper directly*: smaller dep but forces us to write the request
  builder and SSE parser ourselves.
- *isahc / ureq*: lack first-class `Stream` integration in the version
  we'd use, making the streaming path awkward.

## 6. Structured logging setup (both crates)

**Decision**: `tracing` + `tracing-subscriber` configured with
`fmt::layer().json()` for machine-parseable output. HiveClaw logs to
stderr only (it is a foreground process). HiveGUI logs to **both**
stderr (when a TTY is attached) and a rotating file under the
platform's per-user app-data directory, using `tracing-appender`'s
`RollingFileAppender::new(Rotation::DAILY, …)`. The app-data directory
is resolved via the `directories` crate's `ProjectDirs::data_local_dir()`.

**Rationale**: This matches FR-012b verbatim and gives constitution
Principle VI its required correlation-id / operation / outcome / duration
fields via `tracing`'s span model. `tracing-appender` ships from the
`tokio-rs` org and is the conventional companion to `tracing-subscriber`
for file output. `directories` is the standard cross-platform locator
for per-user dirs (handles XDG on Linux, `~/Library/Application Support`
on macOS).

**Alternatives considered**:
- *slog*: capable but the broader Rust ecosystem (axum, tower-http,
  reqwest) instruments against `tracing`, so picking `tracing` removes
  a glue layer.
- *Bespoke writer*: more code, no upside.

## 7. Configuration injection

**Decision**: All runtime knobs are read from environment variables at
process start; no config files in v1. The variables are:

- HiveClaw: `HIVECLAW_BIND_ADDR` (default `127.0.0.1:8686`),
  `HIVECLAW_LOG_LEVEL` (default `info`).
- HiveGUI: `HIVECLAW_URL` (default `http://127.0.0.1:8686`),
  `HIVEGUI_LOG_LEVEL` (default `info`), `HIVEGUI_LOG_DIR` (defaults to
  `ProjectDirs::data_local_dir()/logs`).

Both crates parse env once in `main.rs` via a small `config::Config::from_env()`
function returning `Result<Config, ConfigError>`. No `clap`, no `figment`.

**Rationale**: FR-012 mandates env-injected secrets/connection config and
prohibits committed secrets. A single env-parse function is the simplest
thing that works; adding a config-file layer or a CLI parser violates
Principle V (no future-proofing without a current consumer). Defaults are
chosen so a fresh checkout works with zero env vars, satisfying SC-005.

**Alternatives considered**:
- *clap-based CLI*: nice ergonomics but no current consumer requires
  flags over env vars.
- *TOML config file*: would force a documented schema and a search-path
  story for v1 with no benefit.

## 8. Testing strategy (the compromise)

**Decision**:

- **HiveClaw**: full contract coverage via `reqwest`-driven integration
  tests in `crates/hiveclaw/tests/`. Each test spawns the axum app on an
  ephemeral port (`tokio::net::TcpListener::bind("127.0.0.1:0")`) inside
  the test process, hits it with reqwest, and asserts on the response
  body (sync) or the parsed SSE stream (streaming). No mock layer — the
  real handler runs.
- **HiveGUI client layer** (`crates/hivegui/src/client/`): tested against
  a small in-process axum mock server that re-emits canned OpenResponses
  payloads. This exercises the reqwest + eventsource-stream pipeline
  without depending on HiveClaw.
- **HiveGUI UI layer** (`crates/hivegui/src/ui/`): **not covered by
  automated tests in v1.** A `version_smoke.rs` test confirms the binary
  starts and exits non-zero on bad config; the gpui-rendered window is
  exercised manually per the quickstart. This is the explicit testing
  compromise referenced by plan.md.
- **HiveGUI model layer** (`crates/hivegui/src/model/`): pure-Rust unit
  tests for the `Conversation` state machine (FR-008a sequential turn,
  retryable flag).

**Rationale**: Constitution Principle II requires test-first for every
production module, but it also accepts that contract tests cover external
interfaces while unit tests cover non-trivial branches. gpui windows do
not have a cheap headless test harness today; forcing one would add a
disproportionate maintenance burden for the v1 scaffold's UI, whose
behavior is mostly layout. Pushing the testable invariants down into the
`model` layer keeps the TDD bar real for the parts that actually carry
logic (turn state, retry flag, sequential-send guard) while making the
UI compromise narrow and documented.

**Alternatives considered**:
- *gpui-driven UI tests via screen-shotting*: heavyweight, flaky, no
  established pattern in the gpui ecosystem yet.
- *Skip the in-process mock and test HiveGUI's client against a live
  HiveClaw*: couples two test suites and slows them down; mocks isolate
  the network-format concern cleanly.

## 9. Workspace-level tooling

**Decision**: Single Cargo workspace `Cargo.toml` at the repo root with
`[workspace]` members `crates/hiveclaw` and `crates/hivegui`, plus a
shared `[workspace.dependencies]` table pinning the versions of every
direct dependency once. `rust-toolchain.toml` pins the stable channel
(`channel = "stable"`) plus `components = ["rustfmt", "clippy"]`. CI runs
`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
and `cargo test --workspace`.

**Rationale**: Constitution Tech Stack mandates a single workspace. A
shared `workspace.dependencies` table prevents version drift between the
two crates (e.g., both pin the same `tracing` minor) and satisfies the
lint/format gates from Principle I and Workflow Quality Gates.

**Alternatives considered**:
- *Separate Cargo manifests per crate without `[workspace]`*: would
  fragment the lock file and force per-crate dependency review.
- *Single-crate "monocrate" with bin targets*: blurs the deployable-unit
  boundary the constitution wants the workspace member crates to make
  visible.

## 10. Datastore deferral (sled & SQLite)

**Decision**: Neither `sled` nor `rusqlite` is pulled into v1's
`Cargo.toml` dependency tree. They will be added by the first feature
that actually writes state.

**Rationale**: Spec Assumption explicitly states v1 has no persistent
state — conversation history lives only in HiveGUI memory for the
session. Pulling in unused datastore crates inflates the build, the
review surface, and the security-scan footprint without buying anything
right now. This matches Principle V ("Add them when the second concrete
caller arrives, not before"). When a later feature first writes state,
its plan will record the decision (sled vs SQLite) and add the dep then.

**Alternatives considered**:
- *Pull both in at v1 to "have them ready"*: textbook premature
  abstraction.
- *Pull in one but not the other*: same problem, half-measured.

## Open questions

None. All Technical Context entries in `plan.md` are resolved by the
constitution, the clarifications in `spec.md`, or the decisions above.
Phase 1 may proceed.
