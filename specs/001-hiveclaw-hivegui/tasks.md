---
description: "Task list for HiveClaw Agent & HiveGUI Client (Initial Scaffold)"
---

# Tasks: HiveClaw Agent & HiveGUI Client (Initial Scaffold)

**Input**: Design documents from `/specs/001-hiveclaw-hivegui/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/openresponses-v1.md, quickstart.md

**Tests**: Test tasks ARE included. Test-First Development is constitution-mandated (Principle II), and the plan and research explicitly sequence contract / unit tests before implementation. Tests MUST be written and observed failing before the implementation tasks they cover.

**Organization**: Tasks are grouped by user story. User Story 1 stands the scaffold up; User Story 2 implements the conversation pipeline end-to-end; User Stories 3 and 4 add the Day+1 and Hour+1 tool-series navigation.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Which user story this task belongs to (US1, US2, US3, US4)
- File paths are repository-root relative

## Path Conventions

Single Cargo workspace at the repo root:

- Workspace manifest: `Cargo.toml`, `rust-toolchain.toml`
- HiveClaw crate: `crates/hiveclaw/`
- HiveGUI crate: `crates/hivegui/`

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Stand up the Cargo workspace, toolchain pin, and per-crate skeletons.

- [X] T001 Create workspace manifest `Cargo.toml` at repo root with `[workspace]` members `crates/hiveclaw` and `crates/hivegui`, and a `[workspace.dependencies]` table pinning shared deps (`axum = "0.7"`, `tokio = { version = "1", features = ["full"] }`, `tower-http`, `serde`, `serde_json`, `tracing`, `tracing-subscriber`, `tracing-appender`, `chrono`, `uuid`, `thiserror`, `anyhow`, `reqwest`, `eventsource-stream`, `directories`, `gpui`) per `research.md` §1, §5, §6, §9
- [X] T002 [P] Create `rust-toolchain.toml` at repo root pinning `channel = "stable"` with `components = ["rustfmt", "clippy"]` per `research.md` §9
- [X] T003 [P] Create `.gitignore` at repo root excluding `/target`, `Cargo.lock` policy preserved (committed for binary crates), and editor noise
- [X] T004 [P] Create `crates/hiveclaw/Cargo.toml` declaring a binary crate named `hiveclaw` with `version = "0.1.0"`, edition 2021, dependencies pulled from `[workspace.dependencies]` (axum, tokio, tower-http, serde, serde_json, tracing, tracing-subscriber, chrono, uuid, thiserror, anyhow)
- [X] T005 [P] Create `crates/hivegui/Cargo.toml` declaring a binary crate named `hivegui` with `version = "0.1.0"`, edition 2021, dependencies pulled from `[workspace.dependencies]` (gpui, tokio, reqwest with `rustls-tls`+`stream` features, eventsource-stream, serde, serde_json, tracing, tracing-subscriber, tracing-appender, directories, chrono, uuid, thiserror, anyhow)
- [X] T006 [P] Create empty crate roots so the workspace builds: `crates/hiveclaw/src/main.rs` and `crates/hiveclaw/src/lib.rs` (stubs); `crates/hivegui/src/main.rs` and `crates/hivegui/src/lib.rs` (stubs). Confirm `cargo build --workspace` succeeds.
- [X] T007 [P] Add CI script `.github/workflows/ci.yml` (or equivalent) running `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` per `research.md` §9 and constitution Workflow Quality Gates

**Checkpoint**: Workspace skeleton builds cleanly with `cargo build --workspace`.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Configuration parsing, structured-logging setup, and the zh-CN string registry. These are imported by every user story's binary entrypoint and must exist before any story can wire its `main.rs`.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [X] T008 [P] Implement `crates/hiveclaw/src/config.rs` exposing `Config::from_env() -> Result<Config, ConfigError>` reading `HIVECLAW_BIND_ADDR` (default `127.0.0.1:8686`) and `HIVECLAW_LOG_LEVEL` (default `info`) per `research.md` §7
- [X] T009 [P] Implement `crates/hivegui/src/config.rs` exposing `Config::from_env() -> Result<Config, ConfigError>` reading `HIVECLAW_URL` (default `http://127.0.0.1:8686`), `HIVEGUI_LOG_LEVEL` (default `info`), and `HIVEGUI_LOG_DIR` (default resolved via `directories::ProjectDirs::from("ai", "openclaw", "hivegui").data_local_dir().join("logs")`) per `research.md` §7
- [X] T010 [P] Implement `crates/hiveclaw/src/logging.rs` initialising `tracing-subscriber` with `fmt::layer().json()` writing to stderr, honouring `HIVECLAW_LOG_LEVEL` per `research.md` §6 and constitution Principle VI
- [X] T011 [P] Implement `crates/hivegui/src/logging.rs` initialising `tracing-subscriber` with `fmt::layer().json()` writing to both stderr (when a TTY is attached) and a `tracing_appender::rolling::daily(log_dir, "hivegui.log")` file appender per FR-012b and `research.md` §6
- [X] T012 [P] Create `crates/hivegui/src/ui/strings_zh.rs` declaring all v1 user-facing zh-CN constants (home title, Day+1 section label `"Day+1 工具"`, Hour+1 section label `"Hour+1 工具"`, empty-state `"暂无工具"`, in-progress `"等待 HiveClaw 回复…"`, retry button `"重试"`, unreachable error `"HiveClaw 不可达，请检查服务是否运行"`, send placeholder `"输入你的问题…"`, etc.) per FR-012a and `quickstart.md` §4–§6
- [X] T013 Update `crates/hiveclaw/src/lib.rs` to declare `pub mod config; pub mod logging; pub mod version;` so integration tests can re-use the public surface (`version` module is implemented in US1)
- [X] T014 Update `crates/hivegui/src/lib.rs` to declare `pub mod config; pub mod logging; pub mod version; pub mod model; pub mod client; pub mod ui;` (story phases populate the submodules)

**Checkpoint**: Both crates can parse env config and initialise structured logging in isolation; zh-CN string registry exists.

---

## Phase 3: User Story 1 - Stand up the two-project scaffold (Priority: P1) 🎯 MVP

**Goal**: From a fresh checkout, HiveClaw starts as a placeholder process that logs its name + version, and HiveGUI launches a native desktop window whose home screen shows the conversation entry and the two helper-tool sections (Day+1, Hour+1).

**Independent Test**: After `cargo build --workspace`, running `cargo run -p hiveclaw` prints a structured log line containing `bind_addr` and `version` and listens on the bind address; running `cargo run -p hivegui` opens a native window whose home screen renders the conversation entry plus two sections labelled `Day+1 工具` and `Hour+1 工具`. Per `quickstart.md` §3 and §4 / spec acceptance scenarios 1, 2.

### Tests for User Story 1 ⚠️ (write before implementation, observe red)

- [X] T015 [P] [US1] Smoke test `crates/hiveclaw/tests/version_smoke.rs` — spawns the `hiveclaw` binary via `assert_cmd` (or `std::process::Command`), asserts stderr contains a JSON log line with `target=="hiveclaw"`, `fields.version=="0.1.0"`, and `fields.message=="HiveClaw listening"`, then sends SIGINT and asserts exit status is success (FR-002)
- [X] T016 [P] [US1] Smoke test `crates/hivegui/tests/version_smoke.rs` — spawns the `hivegui` binary with `HIVEGUI_HEADLESS=1` env (handler added in T021), asserts stderr contains a JSON log line `fields.version=="0.1.0"` and that the process exits 0 when the headless flag is set after window-init succeeds

### Implementation for User Story 1

- [X] T017 [P] [US1] Implement `crates/hiveclaw/src/version.rs` exposing `pub const NAME: &str = "HiveClaw";` and `pub fn version() -> &'static str { env!("CARGO_PKG_VERSION") }` per FR-002
- [X] T018 [P] [US1] Implement `crates/hivegui/src/version.rs` exposing `pub const NAME: &str = "HiveGUI";` and `pub fn version() -> &'static str { env!("CARGO_PKG_VERSION") }`
- [X] T019 [US1] Implement `crates/hiveclaw/src/http/mod.rs` building an `axum::Router` with the placeholder routes (real `/v1/responses` handler lands in US2 — for US1, mount a `GET /healthz` that returns `200 OK` with body `"ok"` so the binary genuinely serves traffic) and a `tower_http::trace::TraceLayer` per `research.md` §1
- [X] T020 [US1] Implement `crates/hiveclaw/src/main.rs` — parse `Config::from_env()`, init logging via `logging::init()`, log `HiveClaw listening` with `version` and `bind_addr` fields, bind a `tokio::net::TcpListener` on `config.bind_addr`, serve the router from T019 with `axum::serve`, and exit cleanly on SIGINT (FR-002)
- [X] T021 [US1] Implement `crates/hivegui/src/ui/app.rs` exposing `HiveGuiApp` (gpui `App` setup, window creation, root view registration); accept an env-only test hook `HIVEGUI_HEADLESS=1` that runs the init path through window construction and then exits 0 so T016 can run in CI
- [X] T022 [US1] Implement `crates/hivegui/src/ui/home.rs` — gpui view rendering the home surface with a conversation entry placeholder and two section entries reading their labels from `strings_zh::DAY_PLUS_ONE_LABEL` and `strings_zh::HOUR_PLUS_ONE_LABEL`. v1 may render the sections as inert (clickable hooks come in US3/US4); the labels MUST be present.
- [X] T023 [US1] Implement `crates/hivegui/src/ui/mod.rs` re-exporting `app`, `home`, `strings_zh` and any cross-view scaffolding
- [X] T024 [US1] Implement `crates/hivegui/src/main.rs` — parse `Config::from_env()`, init logging via `logging::init()`, log a JSON line with `version`, instantiate `HiveGuiApp` from T021, and run the gpui event loop

**Checkpoint**: `cargo run -p hiveclaw` and `cargo run -p hivegui` both start successfully; HiveGUI's home screen shows the two section labels. T015 and T016 pass. Spec acceptance scenarios US1 #1 and #2 are satisfied.

---

## Phase 4: User Story 2 - Converse with HiveClaw from HiveGUI (Priority: P2)

**Goal**: The engineer types a question in HiveGUI's conversation surface, HiveGUI forwards it to HiveClaw via `POST /v1/responses`, and HiveClaw's reply renders in the same thread — in both synchronous and streaming modes — with single-pending-turn enforcement and manual retry on failure.

**Independent Test**: With HiveClaw running, send a message from HiveGUI's conversation surface; observe the message appear marked as sent by the engineer, the in-progress indicator render, and HiveClaw's stub reply appear below within ~3s. Stop HiveClaw mid-session, send a follow-up, observe the failed turn with a visible `重试` affordance. Per `quickstart.md` §4 / §6 and spec acceptance scenarios US2 #1–#3 + edge cases.

### Tests for User Story 2 ⚠️ (write before implementation, observe red)

- [X] T025 [P] [US2] Contract test `crates/hiveclaw/tests/contract_responses_sync.rs` — spawn the axum app on `127.0.0.1:0`, POST `{"model":"openclaw:hiveclaw-placeholder-v1","input":"hello"}`, assert response status 200, `Content-Type: application/json`, `X-Request-Id` present, and body matches the synchronous schema in `contracts/openresponses-v1.md` §Response — synchronous (id, object=="response", created, model echoed, status=="completed", output[0].content[0].type=="output_text", usage fields all present and `total == input + output`). Also assert SC-006 p95 < 200ms across 50 sequential requests on warm loopback.
- [X] T026 [P] [US2] Contract test `crates/hiveclaw/tests/contract_responses_streaming.rs` — POST with `"stream": true`, assert `Content-Type: text/event-stream` and `Cache-Control: no-store`, parse SSE frames, assert ordered events `response.created` → one-or-more `response.output_text.delta` → `response.completed` → `data: [DONE]` terminator. Assert concatenated deltas equal the final `response.completed` `output[0].content[0].text`. Assert SC-006 time-to-first-event p95 < 200ms across 50 sequential requests.
- [X] T027 [P] [US2] Contract test `crates/hiveclaw/tests/contract_responses_validation.rs` — for each row in the validation table of `contracts/openresponses-v1.md`, assert the documented status code and JSON error envelope (`{"error":{"type":"invalid_request_error","message":"..."}}`)
- [X] T028 [P] [US2] Unit test `crates/hivegui/tests/conversation_sequential_send.rs` — exercise the `Conversation` state machine: `send_user_message` succeeds when idle; second `send_user_message` while `pending.is_some()` returns `BusyError`; `record_assistant_reply` moves the pending turn to `Delivered` and clears `pending`; `record_failure` moves it to `Failed { retryable: true }`; `retry` produces a new `PendingTurnId` with the same author content; `dismiss_failure` clears the failure without re-sending. Invariants I1–I4 and T1–T4 from `data-model.md`.
- [X] T029 [P] [US2] Client test `crates/hivegui/tests/client_sync_against_mock.rs` — spawn a small axum mock server returning canned synchronous OpenResponses JSON; assert `client::sync::send` parses the response and returns the assistant text
- [X] T030 [P] [US2] Client test `crates/hivegui/tests/client_streaming_against_mock.rs` — spawn a mock server emitting the four canonical SSE events (`response.created`, two `response.output_text.delta`, `response.completed`, `[DONE]`); assert `client::streaming::send` yields the deltas in order via its `Stream`, ends after `response.completed`, and ignores unknown event types (forward-compatibility note in the contract §Forward-compatibility)

### Implementation for User Story 2

#### HiveClaw side

- [X] T031 [P] [US2] Implement `crates/hiveclaw/src/openresponses/mod.rs` defining the wire `serde` structs for the request (`OpenResponsesRequest`, `Input` enum for the string-vs-array form, optional fields) and the response (`OpenResponse`, `OutputItem`, `ContentItem`, `Usage`), plus the `ErrorEnvelope` shape — all matching `contracts/openresponses-v1.md` §Request body and §Response — synchronous
- [X] T032 [P] [US2] Implement `crates/hiveclaw/src/openresponses/stub.rs` producing the deterministic placeholder reply (`"HiveClaw 占位回复：已收到你的请求。"`), token counts, and — for streaming — a 2–4 chunk delta sequence spaced by `tokio::time::sleep(Duration::from_millis(<10ms))` per `research.md` §3 (total stub overhead < 50ms to keep SC-006 trivially within budget)
- [X] T033 [US2] Implement `crates/hiveclaw/src/http/responses.rs` — the `POST /v1/responses` handler. Validate `Content-Type: application/json` and the body via `serde` extractors; on validation failure return the documented `4xx` JSON envelope from `contracts/openresponses-v1.md`. Branch on `stream`: synchronous returns `Json<OpenResponse>` with `X-Request-Id`; streaming returns `axum::response::sse::Sse` emitting `response.created` → deltas → `response.completed` → `[DONE]` per the contract. Echo or generate `X-Request-Id`; set `Cache-Control: no-store` on the SSE branch.
- [X] T034 [US2] Wire the handler into `crates/hiveclaw/src/http/mod.rs` at `POST /v1/responses` and add `tower_http::request_id::SetRequestIdLayer` so missing `X-Request-Id` headers get a UUID v4 generated at the layer boundary
- [X] T035 [US2] Emit the per-request structured log on response completion in the handler / a response-completion `tower` middleware: fields `request_id`, `operation="responses.create"`, `outcome`, `duration_ms`, `stream`, `status_code` per `contracts/openresponses-v1.md` §Logging contract. Logs MUST NOT contain `input`, `instructions`, `output`, or any delta fragment.

#### HiveGUI side

- [X] T036 [P] [US2] Implement `crates/hivegui/src/model/conversation.rs` with the `Conversation`, `ConversationTurn`, `TurnContent`, `TurnStatus`, `Author`, `TurnId`, `PendingTurnId`, `BusyError`, `RetryError`, and `TurnError` types per `data-model.md`. The `Conversation` public API MUST enforce invariants I1–I4 (single pending turn) and `ConversationTurn` MUST follow the state machine T1–T4.
- [X] T037 [P] [US2] Implement `crates/hivegui/src/model/mod.rs` re-exporting `conversation` and a `pub fn sanitize_user_input(raw: &str) -> String` that trims, normalises line endings, and rejects control characters other than `\n`, `\t` per FR-011
- [X] T038 [P] [US2] Implement `crates/hivegui/src/client/mod.rs` building a `reqwest::Client` (with `rustls-tls` and `stream` features enabled) from `Config::hiveclaw_url`, plus shared request types re-using the wire structs (either duplicate from `hiveclaw::openresponses` or define a minimal client-side copy — pick duplicate to keep `hivegui` free of a HiveClaw dep, per Principle V)
- [X] T039 [P] [US2] Implement `crates/hivegui/src/client/sync.rs` exposing `pub async fn send(client: &Client, url: &Url, req: OpenResponsesRequest, request_id: Uuid) -> Result<AssistantReply, ClientError>` that POSTs with `Content-Type: application/json`, sets `X-Request-Id`, and parses the synchronous JSON response
- [X] T040 [P] [US2] Implement `crates/hivegui/src/client/streaming.rs` exposing `pub fn send(...) -> impl Stream<Item = Result<StreamingEvent, ClientError>>` that POSTs with `stream: true`, parses SSE frames via `eventsource-stream`, decodes the `data:` JSON for each known event type, ignores unknown event types (forward-compatibility), and terminates on `data: [DONE]` per the contract
- [X] T041 [US2] Implement `crates/hivegui/src/ui/conversation.rs` — gpui view for the conversation surface. Render `Conversation::turns()` in chronological order with speaker attribution; show the in-progress indicator `strings_zh::IN_PROGRESS` while `Conversation::is_busy()`; disable the send action while busy (FR-008a). On send, sanitize input, call `Conversation::send_user_message`, dispatch a streaming or sync client call (default streaming for incremental render per FR-008), and on each delta update the pending turn's `TurnContent::AssistantText.buffer`. On stream completion call `Conversation::record_assistant_reply`; on transport error call `Conversation::record_failure` and render the zh-CN error + `重试` button. Clicking `重试` calls `Conversation::retry` and re-dispatches.
- [X] T042 [US2] Update `crates/hivegui/src/ui/home.rs` so the conversation entry navigates into the `ConversationView` from T041; the `HiveGuiApp` (T021) must own the `Conversation` model instance and keep it alive across navigation so in-flight turns survive section switches (SC-004 / FR-010 / cross-entity rule C1)
- [X] T043 [US2] Plumb `conversation_id` as the `tracing` span correlation field on every outbound client call (cross-entity rule C3) so HiveGUI's JSON logs carry `conversation_id`, `request_id`, `operation`, `outcome`, `duration_ms` per FR-012b and constitution Principle VI

**Checkpoint**: T025–T030 pass green. The engineer can send a message and see HiveClaw's reply render incrementally (streaming) or atomically (sync). Stopping HiveClaw produces a `重试`-bearing failed turn. Spec acceptance scenarios US2 #1–#3 and edge cases "HiveClaw unreachable mid-conversation" / "Very long agent reply" satisfied.

---

## Phase 5: User Story 3 - Open a Day+1 helper tool (Priority: P3)

**Goal**: HiveGUI's Day+1 section is navigable and renders the documented zh-CN empty state in v1; the launch path is forward-compatible so a future feature can plug a `HelperTool` implementation in without touching the navigation shell.

**Independent Test**: From HiveGUI's home screen, click the Day+1 section; observe a list view that renders the `strings_zh::EMPTY_TOOLS` empty-state. Navigate back to the conversation; any in-flight conversation state is preserved (SC-004). Per spec acceptance scenarios US3 #1 and #2.

### Tests for User Story 3 ⚠️

- [X] T044 [P] [US3] Unit test `crates/hivegui/tests/tool_series_empty_state.rs` — assert `ToolSeries::for_kind(ToolSeriesKind::DayPlusOne).tools.is_empty()` holds in v1, and that `ToolSeries::display_name_zh()` returns the zh-CN label declared in `strings_zh` (invariant S1)

### Implementation for User Story 3

- [X] T045 [P] [US3] Implement `crates/hivegui/src/model/tools.rs` defining `ToolSeriesKind { DayPlusOne, HourPlusOne }`, `ToolSeries`, `HelperTool`, `HelperToolId`, and the `HelperToolSurface` trait per `data-model.md`. v1 ships zero `HelperTool` constructors (invariant H3). `ToolSeries::for_kind` returns instances with `tools: vec![]` for both kinds.
- [X] T046 [US3] Implement `crates/hivegui/src/ui/tools_section.rs` — a gpui view that renders any `ToolSeries`. When `tools.is_empty()`, render `strings_zh::EMPTY_TOOLS`. When non-empty (future-compatible), render the list with name + description and a launch action that opens the tool's surface via `HelperToolSurface`. Distinguish "no tools yet" from "loading failed" with distinct copy slots even though v1 only exercises the empty-list branch (invariant S1).
- [X] T047 [US3] Wire a Day+1 entry in `crates/hivegui/src/ui/home.rs` (extending T022/T042) that navigates to `ToolsSectionView::new(ToolSeriesKind::DayPlusOne)` and back, preserving the owning app's `Conversation` model across the navigation (SC-004 / FR-010 / cross-entity rule C1)

**Checkpoint**: Day+1 section is reachable from home and renders the zh-CN empty state; conversation state survives a round-trip from home → Day+1 → home → conversation. T044 passes.

---

## Phase 6: User Story 4 - Open an Hour+1 helper tool (Priority: P3)

**Goal**: Symmetric to US3 for the Hour+1 series.

**Independent Test**: From HiveGUI's home screen, click the Hour+1 section; observe the empty-state render; navigate away and back without losing conversation state. Per spec acceptance scenarios US4 #1 and #2.

**Dependency**: US4 reuses `crates/hivegui/src/model/tools.rs` and `crates/hivegui/src/ui/tools_section.rs` from US3. The model already defines `ToolSeriesKind::HourPlusOne`; the view is generic over `ToolSeries`. US4 adds the home-screen wiring and a test against the Hour+1 kind.

### Tests for User Story 4 ⚠️

- [X] T048 [P] [US4] Unit test `crates/hivegui/tests/tool_series_hour_plus_one.rs` — assert `ToolSeries::for_kind(ToolSeriesKind::HourPlusOne).tools.is_empty()` holds, and that its `display_name_zh()` returns the Hour+1 zh-CN label declared in `strings_zh` (invariant S1)

### Implementation for User Story 4

- [X] T049 [US4] Wire an Hour+1 entry in `crates/hivegui/src/ui/home.rs` (extending T022/T042/T047) that navigates to `ToolsSectionView::new(ToolSeriesKind::HourPlusOne)` and back, preserving the owning app's `Conversation` model across the navigation (SC-004 / FR-010)

**Checkpoint**: Both tool sections are reachable, render empty-state, and preserve conversation state. T048 passes. All four user stories are independently exercisable.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, end-to-end validation, performance verification, and final hygiene.

- [X] T050 [P] Add a top-level `README.md` whose body is a thin pointer to `specs/001-hiveclaw-hivegui/quickstart.md` plus the workspace layout (do not duplicate content; link out)
- [X] T051 [P] Add `docs/quickstart.md` as a copy or symlink of the canonical quickstart so the file path referenced from `plan.md` §Project Structure exists (per `plan.md` `docs/quickstart.md` line)
- [X] T052 Run the full `quickstart.md` flow end-to-end on a clean checkout: build, run HiveClaw, run HiveGUI, send a message, stop HiveClaw, observe `重试`, restart, click retry, observe reply. Confirm SC-001 (under 15 minutes from clean machine) and SC-005 (zero source edits).
- [X] T053 Performance check: run `crates/hiveclaw/tests/contract_responses_sync.rs` and `..._streaming.rs` under release-mode (`cargo test --workspace --release`) and confirm SC-006 thresholds (sync total p95 < 200ms; streaming TTFB p95 < 200ms) hold on the target machine. Record the measured numbers in a brief note inside each test or in a sibling `bench_notes.md`.
- [X] T054 [P] Security pass — verify no secrets are committed (grep for `HIVECLAW_URL`, `token`, `key`, `secret` in source); verify HiveClaw rejects malformed request bodies at the axum extractor boundary (T027 already covers this — confirm coverage in code review); confirm HiveGUI logs do not serialise `input`, `output`, or delta fragments (FR-012b, cross-entity rule C2)
- [X] T055 [P] Run the workspace quality gates and ensure they pass clean: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (constitution Workflow Quality Gates)
- [X] T056 [P] Confirm no `unused` warnings and no dead code suppressions outside genuine trait stubs (`HelperToolSurface` is the only intentional stub per invariant H3); document that single exception inline if clippy flags it

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately.
- **Foundational (Phase 2)**: Depends on Setup. BLOCKS all user stories.
- **User Story 1 (Phase 3)**: Depends on Foundational. MVP — implement first.
- **User Story 2 (Phase 4)**: Depends on Foundational. Independent of US1 in principle (it has its own model + UI), but in practice it extends the same `HiveGuiApp` shell that US1 stands up. Implement after US1 to share the shell. Tests T025–T030 MUST be red before implementation tasks T031–T043 begin (constitution Principle II).
- **User Story 3 (Phase 5)**: Depends on Foundational. Independent of US2's conversation pipeline; only depends on the home-screen shell from US1 (T022) and the gpui navigation pattern from US2 (T042) to stay consistent. Test T044 MUST be red before T045–T047.
- **User Story 4 (Phase 6)**: Depends on US3 for `model/tools.rs` and `ui/tools_section.rs`. Independent of US2 and reuses the generic tools view. Test T048 MUST be red before T049.
- **Polish (Phase 7)**: Depends on US1–US4 being complete (or on the subset shipped in the MVP slice).

### User Story Dependencies

- **US1 (P1)**: No story dependencies.
- **US2 (P2)**: No story dependencies in principle; in practice reuses the `HiveGuiApp` shell from US1.
- **US3 (P3)**: Reuses the home-screen shell from US1; no other story dependency.
- **US4 (P3)**: Depends on US3's `model/tools.rs` (T045) and `ui/tools_section.rs` (T046).

### Within Each User Story

- All tests in the story's "Tests for User Story N" section MUST be written and observed RED before any implementation task in that story begins (constitution Principle II).
- Inside a story: wire types / models before services; services before endpoints / views; core implementation before integration glue.

### Parallel Opportunities

- **Setup**: T002, T003, T004, T005 can run in parallel after T001 lands the workspace manifest. T006 and T007 can run in parallel once T002–T005 are in.
- **Foundational**: T008–T012 are all [P] — five files in five different paths. T013 and T014 sequence after their respective modules exist.
- **US1 tests**: T015 and T016 can run in parallel (different test files, different crates).
- **US1 implementation**: T017 and T018 can run in parallel (different `version.rs` files); T019–T024 then sequence by file-dependency.
- **US2 tests**: T025, T026, T027, T028, T029, T030 can all run in parallel — six different test files.
- **US2 implementation**: HiveClaw side (T031–T035) and HiveGUI side (T036–T043) can be split across two contributors and run in parallel. Within each side, the [P]-marked tasks parallelise.
- **US3 vs US4**: Once US3's T045 + T046 are in, US4's T048 (test) and US3's T047 (home wiring) can proceed in parallel; T049 sequences after T048 is red.

---

## Parallel Example: User Story 2 tests

```bash
# Launch all US2 tests together (write them red, then implement):
Task: "Contract test sync in crates/hiveclaw/tests/contract_responses_sync.rs"
Task: "Contract test streaming in crates/hiveclaw/tests/contract_responses_streaming.rs"
Task: "Contract test validation in crates/hiveclaw/tests/contract_responses_validation.rs"
Task: "Conversation state-machine unit test in crates/hivegui/tests/conversation_sequential_send.rs"
Task: "Client sync test in crates/hivegui/tests/client_sync_against_mock.rs"
Task: "Client streaming test in crates/hivegui/tests/client_streaming_against_mock.rs"
```

## Parallel Example: User Story 2 implementation (split across contributors)

```bash
# HiveClaw side — contributor A:
Task: "OpenResponses wire types in crates/hiveclaw/src/openresponses/mod.rs"
Task: "OpenResponses stub generator in crates/hiveclaw/src/openresponses/stub.rs"

# HiveGUI side — contributor B (in parallel):
Task: "Conversation model in crates/hivegui/src/model/conversation.rs"
Task: "Sanitiser + model re-exports in crates/hivegui/src/model/mod.rs"
Task: "reqwest client base in crates/hivegui/src/client/mod.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup.
2. Complete Phase 2: Foundational — config + logging + zh-CN strings.
3. Complete Phase 3: User Story 1 — both binaries start, HiveGUI home shows sections.
4. **STOP and VALIDATE**: Run `cargo test -p hiveclaw --test version_smoke` and `cargo test -p hivegui --test version_smoke`. Launch both binaries by hand. Confirm spec acceptance scenarios US1 #1 and #2.
5. This is the smallest demoable slice — the two projects exist as distinct buildable units (FR-001, FR-005), HiveClaw reports name + version (FR-002), HiveGUI's home renders both tool sections (FR-006).

### Incremental Delivery

1. Ship MVP (US1) → demoable scaffold.
2. Add US2 → demoable conversation surface end-to-end (the primary user interaction).
3. Add US3 → Day+1 section reachable with empty-state.
4. Add US4 → Hour+1 section reachable with empty-state.
5. Run Polish (Phase 7) and tag v0.1.0.

### Parallel Team Strategy

With three contributors after Phase 2 lands:

- **Contributor A**: HiveClaw side of US2 — handler, wire types, stub, contract tests (T025–T027, T031–T035).
- **Contributor B**: HiveGUI side of US2 — model, client, conversation UI, unit + mock tests (T028–T030, T036–T043).
- **Contributor C**: US1 finalisation + US3 + US4 — home-screen shell, tool series model + view (T015–T024, T044–T049).

Stories integrate through the `HiveGuiApp` shell owned by US1; the two contributors on US2 sync on the wire format defined in `contracts/openresponses-v1.md`.

---

## Notes

- [P] tasks = different files, no dependencies on incomplete tasks.
- [Story] label maps each task to its user story for traceability.
- Tests MUST be written and observed red before the implementation they cover (constitution Principle II).
- Commit after each task or logical group; keep zh-CN strings consolidated in `strings_zh.rs` so review can verify FR-012a in one place.
- Stop at any checkpoint to validate the story independently against its Independent Test.
- Avoid: vague tasks, same-file conflicts, cross-story dependencies that break story independence (the only sanctioned cross-story dependency is US4 → US3 for the shared tools model + view, and it is explicitly documented above).
