# Feature Specification: HiveClaw Agent & HiveGUI Client (Initial Scaffold)

**Feature Branch**: `001-hiveclaw-hivegui`
**Created**: 2026-05-14
**Status**: Draft
**Input**: User description: "我要搭建一个AI Agent项目，它包括两部分：HiveClaw（类似OpenClaw的AI Agent，用于Hive离线分析任务的开发和调试）；HiveGUI（HiveClaw的客户端，可与HiveClaw对话，并提供 Day+1 与 Hour+1 两个辅助工具系列）。HiveClaw 先建空项目占位。"

## Clarifications

### Session 2026-05-14

- Q: HiveGUI delivery form factor — desktop / local-web / TUI / CLI? → A: Desktop application (native window on the engineer's machine; no browser, no server tier).
- Q: HiveClaw conversation reply mode — sync, streaming, or both? → A: Both, via OpenResponses HTTP API compatibility. HiveClaw exposes `POST /v1/responses` per the OpenClaw OpenResponses spec (https://docs.openclaw.ai/gateway/openresponses-http-api). The caller selects sync JSON or SSE streaming via the `stream` request field on each call.
- Q: HiveGUI UI language? → A: Chinese only (Simplified, `zh-CN`). All user-facing copy — home screen, section labels, conversation UI, empty-state messages, error messages — MUST be in Simplified Chinese. No i18n framework in v1. Developer-facing strings (logs, exceptions, source identifiers) MAY remain in English.
- Q: Failed-turn retry behaviour? → A: Manual only. A failed conversation turn MUST display an error state with a visible "重试" (Retry) affordance attached to that turn. Nothing re-sends until the engineer activates it. No automatic retry, no silent re-send on reconnect.
- Q: HiveGUI structured-logging destination? → A: Rotating JSON-lines log file in a documented per-user application-data directory, plus stderr when HiveGUI is launched from a terminal. Satisfies constitution Principle VI without a network-bound logging dependency.
- Q: Concurrent in-flight conversation messages? → A: Sequential. At most **one** pending turn at any time. The send action MUST be disabled while a turn is pending; it MUST re-enable only when the pending turn resolves (delivered, failed, or — in the failed case — after the engineer dismisses or manually retries it). No queuing, no concurrent in-flight turns.
- Q: How does constitution Principle IV's "p95 < 200ms" apply to HiveClaw's `/v1/responses`? → A: For **synchronous** requests (`stream: false` or omitted), the budget is **total response time, p95 < 200ms**. For **streaming** requests (`stream: true`), the budget is **time-to-first-event, p95 < 200ms** (total stream duration is unbounded by this gate). Both metrics are measured at HiveClaw's HTTP boundary under representative load.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Stand up the two-project scaffold (Priority: P1)

A data engineer who is starting work on the HiveClaw / HiveGUI product needs
the two projects to exist as separate, independently-buildable units so that
later feature work can land in the correct place without ambiguity. HiveClaw
exists only as a placeholder right now; HiveGUI begins to take shape as the
client.

**Why this priority**: Nothing else in the product roadmap can begin until
the two projects exist as distinct, buildable units with a clear ownership
boundary between them. This is the minimum viable starting point.

**Independent Test**: Can be fully tested by cloning the repository fresh,
running the documented bootstrap steps for each project, and confirming
that (a) the HiveClaw placeholder builds/starts and reports its name and
version, and (b) the HiveGUI client builds/starts and opens to a home
screen showing the two helper-tool series (Day+1, Hour+1) as visible
sections, even if the tools themselves are empty.

**Acceptance Scenarios**:

1. **Given** a fresh checkout of the repository, **When** the engineer
   follows the documented bootstrap steps for HiveClaw, **Then** the
   HiveClaw placeholder process starts, identifies itself by name and
   version, and exits cleanly with success.
2. **Given** a fresh checkout of the repository, **When** the engineer
   follows the documented bootstrap steps for HiveGUI, **Then** the
   HiveGUI client launches and presents a home screen that lists the
   two helper-tool series as navigable sections.
3. **Given** the HiveGUI client is running and HiveClaw is reachable,
   **When** the engineer opens the conversation surface and sends a
   message, **Then** HiveGUI forwards the message to HiveClaw and
   displays HiveClaw's reply in the same conversation thread.
4. **Given** HiveClaw is unreachable, **When** the engineer opens the
   conversation surface and sends a message, **Then** HiveGUI surfaces a
   clear, actionable error stating that HiveClaw is not reachable and
   how to verify connectivity, without crashing.

---

### User Story 2 - Converse with HiveClaw from HiveGUI (Priority: P2)

A data engineer working on a Hive offline-analysis task wants to use
HiveGUI's conversation surface to ask HiveClaw for help (drafting,
explaining, or debugging Hive SQL and related offline jobs) and to keep a
visible thread of the exchange so the conversation can be reviewed later
in the same session.

**Why this priority**: The conversation surface is the primary user
interaction with the agent. Without it the GUI has no purpose beyond
hosting helper tools.

**Independent Test**: With HiveClaw running as a placeholder that echoes
or stubs replies, an engineer can open HiveGUI, type a question, see
HiveGUI deliver it to HiveClaw, and see HiveClaw's response appear in
the conversation thread within the same session.

**Acceptance Scenarios**:

1. **Given** HiveGUI is open and connected to HiveClaw, **When** the
   engineer sends a message, **Then** the message appears in the thread
   marked as sent by the engineer, and HiveClaw's reply appears below
   it marked as sent by HiveClaw, in order.
2. **Given** an ongoing conversation, **When** the engineer sends a
   follow-up message, **Then** the prior turns remain visible above the
   new turn for the duration of the session.
3. **Given** HiveClaw takes a noticeable amount of time to reply,
   **When** the engineer is waiting, **Then** HiveGUI shows a clear
   in-progress indicator so the engineer knows the request is being
   processed and has not been lost.

---

### User Story 3 - Open a Day+1 helper tool (Priority: P3)

An analyst running daily-batch ("Day+1", i.e. next-day offline) work
wants to open a helper tool from the Day+1 series inside HiveGUI to
accomplish a routine daily-cadence task without leaving the client.

**Why this priority**: The Day+1 series is the first concrete set of
helpers users will reach for once the scaffold and conversation surface
exist. In v1 no Day+1 tools are configured (the series ships empty), so
only the section's navigation, listing, and empty-state behaviour are
in scope. The launch path is specified as a forward-compatible contract
that activates once tools are added in a later feature.

**Independent Test**: An engineer can open HiveGUI, navigate to the
Day+1 section, see a list of tools in that series (initially possibly
empty with a "no tools yet" message), and — once tools are added —
launch one and see its dedicated working surface within the same client.

**Acceptance Scenarios**:

1. **Given** HiveGUI is open, **When** the engineer selects the Day+1
   section, **Then** the section displays the list of tools belonging
   to that series, or an empty-state message if none are configured.
2. **Given** at least one Day+1 tool is configured, **When** the engineer
   selects it, **Then** HiveGUI opens that tool's working surface inside
   the client and the engineer can return to the conversation or to the
   tool list without losing in-flight state.

---

### User Story 4 - Open an Hour+1 helper tool (Priority: P3)

An analyst running hourly-batch ("Hour+1", i.e. next-hour offline) work
wants to open a helper tool from the Hour+1 series inside HiveGUI for
routine hourly-cadence tasks.

**Why this priority**: Symmetric to User Story 3 for the Hour+1 series.
In v1 no Hour+1 tools are configured (the series ships empty), so only
the section's listing and empty-state behaviour are in scope; the launch
path is a forward-compatible contract for later features.

**Independent Test**: An engineer can open HiveGUI, navigate to the
Hour+1 section, see the tools in that series (or an empty-state), and
launch a tool to reach its dedicated working surface inside the client.

**Acceptance Scenarios**:

1. **Given** HiveGUI is open, **When** the engineer selects the Hour+1
   section, **Then** the section displays the list of tools belonging
   to that series, or an empty-state message if none are configured.
2. **Given** at least one Hour+1 tool is configured, **When** the engineer
   selects it, **Then** HiveGUI opens that tool's working surface inside
   the client and the engineer can return to the conversation or to the
   tool list without losing in-flight state.

---

### User Story 5 - Compose conversation turns with a text editor and file attachments (Priority: P1)

A data engineer working on a Hive offline-analysis task wants to type
a question into HiveGUI's conversation surface (instead of having only
a stub Send button) and attach one or more files — typically a SQL/HQL
script, a schema fragment, or a small sample data file — alongside the
typed question so HiveClaw receives both the prose and the file content
as the same conversation turn.

**Why this priority**: The current conversation surface has a Send
button bound to no input element, so FR-007 is unsatisfied in practice
— the engineer cannot actually type anything. The file-attachment
capability is the user-requested extension on top of that gap fix.
Treating both together avoids touching the conversation UI twice.

**Independent Test**: With HiveClaw running and HiveGUI open, the
engineer types a question, clicks "添加文件" and picks a `.hql` script,
clicks "发送", and observes the user turn rendering with both the typed
text and an attachment chip (filename + size + MIME), HiveClaw's
placeholder reply landing with the documented `附件：<filename> (<size>,
<mime>)` zh-CN acknowledgement appended, and the send pipeline returning
to idle. Repeating with a file larger than 1 MiB shows an actionable
zh-CN error on the input surface **before** any network call is issued.

**Acceptance Scenarios**:

1. **Given** HiveGUI is open and connected to HiveClaw, **When** the
   engineer types text into the conversation input and clicks "发送",
   **Then** the typed text appears as a User turn marked as sent by the
   engineer, and HiveClaw's reply appears below it in order.
2. **Given** the engineer has typed text **and** attached one or more
   files (each ≤ 1 MiB; total ≤ 4 MiB), **When** the engineer clicks
   "发送", **Then** HiveGUI dispatches a single `POST /v1/responses`
   request that carries the text as an `input_text` content item and
   each file as an `input_file` content item (inline `data:` URI), and
   HiveClaw's placeholder reply acknowledges each attachment by
   filename, size, and MIME.
3. **Given** the editor is empty **and** no files are attached, **When**
   the engineer looks at the Send affordance, **Then** Send is disabled
   and gives no apparent action on click.
4. **Given** the engineer picks a file larger than 1 MiB, **When** the
   file picker returns, **Then** HiveGUI rejects the file inline with a
   zh-CN error before adding it to the pending attachments list, and
   the engineer's typed text remains intact in the editor.

### Edge Cases

- **Client-side window decoration on Linux**: On Linux (Wayland/X11), where server-side window decorations are not universally available, HiveGUI MUST render a custom client-side titlebar providing: (a) a draggable region for moving the window, (b) a double-click gesture to toggle maximise/restore, (c) visible controls for minimise, maximise/restore, and close. The titlebar MUST use a distinct background colour to avoid blending with the content area.
- **HiveClaw unreachable at startup**: HiveGUI MUST start and present
  the helper-tool sections; the conversation surface MUST display a
  clear "agent unreachable" state with guidance, never a crash or blank
  screen.
- **HiveClaw becomes unreachable mid-conversation**: in-flight requests
  MUST fail with an actionable error attached to the conversation turn,
  and the engineer MUST be able to **manually retry** the same turn
  (via a "重试" affordance on the failed turn) once connectivity is
  restored. HiveGUI MUST NOT auto-retry or silently re-send.
- **Helper-tool series is empty**: each series section MUST render an
  explicit empty-state message rather than an empty pane, so the
  engineer can distinguish "no tools configured" from "loading failed".
- **Very long agent reply**: the conversation surface MUST remain
  scrollable and responsive; replies MUST NOT be silently truncated.
- **Engineer switches between conversation and a helper tool**: state
  in each surface MUST persist for the duration of the session so the
  engineer can move back and forth without losing context.
- **Multiple HiveGUI windows pointing at the same HiveClaw**: behaviour
  is out of scope for v1 (single-window assumption); see Assumptions.
- **Oversize file picked**: a file larger than the documented per-file
  limit (1 MiB) MUST be rejected **before** any network call, with an
  actionable zh-CN error attached to the input surface (not to the
  conversation thread, since no turn has been sent yet).
- **Duplicate file attachment**: when the engineer attempts to attach
  the same file (by filename) more than once to the same pending turn,
  the duplicate MUST be rejected with a zh-CN error before it is added
  to the pending attachments list, and the prior attachments MUST remain
  intact. Duplication is detected by exact filename match.
- **Total attachment cap exceeded**: when the running sum of attached
  file sizes would exceed the documented total cap (4 MiB), the
  attempted attachment MUST be rejected with a zh-CN error and the
  prior attachments MUST remain intact and pending.
- **Malformed `data:` URI on the wire**: if HiveGUI ever sends an
  `input_file.file_data` value that is not a `data:<mime>;base64,<payload>`
  URI (or the payload fails to base64-decode), HiveClaw MUST reject the
  whole request at the validation boundary with a `400` and the
  documented JSON error envelope; the failed turn MUST surface a
  retryable zh-CN error in the conversation thread.
- **Empty editor + zero attachments**: the Send affordance MUST be
  disabled. There is no path by which an empty turn reaches HiveClaw.

## Requirements *(mandatory)*

### Functional Requirements

**HiveClaw (placeholder scope only)**

- **FR-001**: The repository MUST contain a HiveClaw project as a
  distinct, independently buildable unit, separate from HiveGUI.
- **FR-002**: HiveClaw MUST start as a placeholder process that
  reports its name (`HiveClaw`) and a version identifier, and exits
  with success when asked to.
- **FR-003**: HiveClaw MUST expose a conversation endpoint that is
  **wire-compatible with the OpenClaw OpenResponses HTTP API**
  (`POST /v1/responses`), accepting the documented request fields
  (`model`, `input`, `instructions`, `stream`, etc.) and producing
  responses that conform to that specification. Both reply modes
  MUST be supported: synchronous JSON when `stream` is false or
  omitted, and Server-Sent Events when `stream` is true (terminating
  with the documented `[DONE]` sentinel). The placeholder
  implementation MAY return stub content, but the wire contract
  MUST match the specification so HiveGUI can consume it without
  agent-specific shims.
- **FR-003a**: HiveClaw MUST accept the OpenResponses **content-array
  form** on `POST /v1/responses`, where each `input` item's `content`
  field is an array of typed content items. v1 MUST recognise three
  content-item types: `input_text` (carrying `text: String`),
  `input_file` (carrying `filename: String` and `file_data: String`
  where `file_data` is a `data:<mime>;base64,<payload>` URI — `file_id`
  is reserved for v1.x and MUST be rejected with the documented error),
  and `input_image` (carrying `image_url: String` as a `data:image/*;
  base64,<payload>` URI). Per-file decoded size MUST be ≤ 1 MiB and the
  total decoded size across all attachments on a single request MUST be
  ≤ 4 MiB; the raw HTTP body is independently capped at 8 MiB at the
  transport layer. All these limits are enforced at the validation
  boundary with the JSON error envelopes documented in
  `contracts/openresponses-v1.md`.
- **FR-004**: HiveClaw MUST emit structured logs for every conversation
  request it handles (request id, outcome, duration), per the project
  constitution's Observability principle.

**HiveGUI (client + helper tool host)**

- **FR-005**: The repository MUST contain a HiveGUI project as a
  distinct, independently buildable **desktop application**, separate
  from HiveClaw. HiveGUI MUST run as a native desktop window on the
  engineer's own machine; it MUST NOT require a browser to use, and
  MUST NOT introduce a server tier.
- **FR-006**: HiveGUI MUST start and present a home surface that
  exposes (a) a conversation entry point, (b) a Day+1 tools section,
  and (c) an Hour+1 tools section.
- **FR-007**: HiveGUI MUST let the engineer send a message to HiveClaw
  and display HiveClaw's reply in the same conversation thread, with
  speaker attribution and chronological ordering.
- **FR-007a**: HiveGUI MUST present a typeable text-editor element on
  the conversation surface, bound to the Send action. The Send action
  MUST be disabled when the editor's text is empty (after sanitisation
  per FR-011) **and** no file attachments are pending; it MUST be
  enabled as soon as either condition becomes non-empty. Submit-on-Enter
  is required; Shift+Enter inserts a literal newline.
- **FR-007b**: HiveGUI MUST allow the engineer to attach one or more
  files to a pending conversation turn via a documented affordance
  ("添加文件"). Attachments MUST be inline-encoded into the outbound
  OpenResponses `input` array as `input_file` content items per FR-003a;
  per-file size limit is `1 MiB`, total limit is `4 MiB`, maximum
  attachment count per turn is **8**. Oversize files, excess
  attachments, and duplicate files (by exact filename match) MUST be
  rejected before the turn is sent, with an actionable zh-CN error
  rendered next to the input surface (not in the conversation thread,
  which is reserved for sent turns).
- **FR-008**: HiveGUI MUST show a visible in-progress indicator while
  a reply is pending, and a clear error state when a reply fails or
  HiveClaw is unreachable. When the underlying request to HiveClaw
  uses streaming (`stream: true`), HiveGUI MUST render the reply
  incrementally as SSE events arrive; when it uses synchronous mode,
  the indicator MUST remain visible until the complete reply lands.
- **FR-008a**: HiveGUI MUST allow **at most one** pending conversation
  turn at any time. The send action MUST be disabled while a turn is
  pending and MUST re-enable only when that turn resolves (delivered,
  failed, or — for a failed turn — after the engineer manually retries
  or dismisses it). HiveGUI MUST NOT queue subsequent sends and MUST
  NOT dispatch concurrent in-flight turns.
- **FR-009**: HiveGUI MUST list the helper tools belonging to each
  series (Day+1, Hour+1) and MUST render an explicit empty-state when
  a series has no configured tools.
- **FR-010**: HiveGUI MUST allow the engineer to open a helper tool's
  working surface in-client and to navigate back to the tool list or
  to the conversation without losing in-flight state in either.
- **FR-011**: HiveGUI MUST validate and sanitise any user-supplied
  input before forwarding it to HiveClaw or to a helper tool, per the
  project constitution's Security Requirements.
- **FR-012**: HiveGUI MUST NOT embed secrets (HiveClaw connection
  credentials, tokens, keys) in committed configuration; it MUST read
  them from environment-injected configuration at runtime.
- **FR-012a**: All user-facing copy in HiveGUI (home screen, section
  labels, conversation surface, empty-state messages, error messages,
  in-progress indicators) MUST be authored in Simplified Chinese
  (`zh-CN`). HiveGUI MUST NOT ship an i18n / localisation framework in
  v1. Developer-facing strings (log lines, exception messages, internal
  identifiers) MAY remain in English.
- **FR-012b**: HiveGUI MUST emit structured logs (one JSON object per
  line) for its long-running session, written to (a) a rotating log
  file in a documented per-user application-data directory, and (b)
  stderr when HiveGUI is launched attached to a terminal. Logs MUST
  include the fields required by constitution Principle VI (correlation
  id, operation, outcome, duration) and MUST NOT contain secrets, full
  request/response payloads, or PII in plain text.

**Day+1 tool series (v1 ships empty)**

- **FR-013**: The Day+1 series MUST be reserved for helper tools whose
  data cadence is the next-day offline batch (T-1 daily). In v1 the
  series ships with **no tools configured**; HiveGUI MUST render the
  documented empty-state for the series. Concrete tools will be added
  in later features.

**Hour+1 tool series (v1 ships empty)**

- **FR-014**: The Hour+1 series MUST be reserved for helper tools whose
  data cadence is the next-hour offline batch (T-1 hourly). In v1 the
  series ships with **no tools configured**; HiveGUI MUST render the
  documented empty-state for the series. Concrete tools will be added
  in later features.

**Users & access**

- **FR-015**: HiveGUI is a **single-user, local tool** in v1. It runs
  on the engineer's own machine and talks to a local or personal
  HiveClaw. There is no user identity, no authentication, and no
  access control surface in v1. Multi-user / shared deployment is
  explicitly out of scope.

### Key Entities

- **Conversation**: a chronological sequence of turns within a single
  HiveGUI session. Attributes: ordered turns, and a single optional
  `pendingTurn` slot (at most one pending turn at any time).
- **Conversation Turn**: a single exchange element — either a message
  authored by the engineer or a reply produced by HiveClaw. Attributes:
  author, timestamp, content, status (pending / delivered / failed),
  and — for failed engineer-authored turns — an explicit "retryable"
  flag that enables the manual Retry affordance.
- **Tool Series**: a named grouping of helper tools by data cadence.
  Two series exist in v1: `Day+1` and `Hour+1`.
- **Helper Tool**: a self-contained working surface inside HiveGUI
  belonging to exactly one Tool Series. Attributes: name, series,
  description, working-surface state.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A new engineer can clone the repository and reach a
  working HiveGUI home screen connected to a running HiveClaw
  placeholder in **under 15 minutes** from a clean machine, following
  only the bootstrap documentation.
- **SC-002**: In an interactive session against the HiveClaw
  placeholder, **95%** of user messages receive a visible reply or a
  visible error within **3 seconds** of being sent.
- **SC-003**: When HiveClaw is unreachable, **100%** of attempted
  conversation sends surface a human-readable, actionable error in the
  conversation thread — none result in a crash, frozen UI, or silent
  failure.
- **SC-004**: An engineer can switch between the conversation surface
  and any helper tool, and back, **without losing in-flight state**
  in either surface, on every attempt within a single session.
- **SC-005**: Both projects (HiveClaw, HiveGUI) build from a clean
  checkout with **zero manual edits** to source-controlled files.
- **SC-006**: HiveClaw's `POST /v1/responses` meets, at p95 under
  representative load on **text-only** requests (i.e., no `input_file`
  or `input_image` content items): **< 200ms total response time** for
  synchronous requests, and **< 200ms time-to-first-event** for
  streaming requests. Total stream duration is not bounded by this
  criterion. Requests carrying any attachment are governed by SC-007
  below.
- **SC-007**: For `POST /v1/responses` requests containing one or more
  `input_file` / `input_image` content items, HiveClaw's p95 (sync mode:
  total response time; streaming mode: time-to-first-event) MUST be
  **< 500ms**, measured at HiveClaw's HTTP boundary, on payloads up to
  the documented `1 MiB`-per-file and `4 MiB`-total limits. The 500ms
  ceiling budgets base64 decoding (~50–80ms for 4 MiB), MIME bookkeeping,
  and per-attachment metadata formatting in the stub reply, in addition
  to the same baseline that SC-006 covers.

## Assumptions

- HiveClaw in this iteration is a **placeholder**: it must satisfy
  the conversation contract enough for HiveGUI to talk to it (so
  HiveGUI is genuinely testable), but it is not expected to do real
  Hive task development or debugging yet — that scope is for a
  later feature.
- The two projects live in **one repository** (the current one), as
  two top-level units. This is consistent with the constitution's
  "at most three projects" simplicity rule (HiveClaw + HiveGUI + an
  optional shared module if one is justified later).
- HiveGUI is a **single-user, single-window, single-session** local
  client in v1 (resolved from clarification): no user identity, no
  authentication, no access control, no multi-window behaviour.
- Engineers run HiveGUI and HiveClaw on machines where both processes
  are reachable to each other via a local or trusted network; remote
  / hosted deployment is out of scope for v1.
- The user described "类似 OpenClaw" — we treat OpenClaw as a stylistic
  reference (an agent for a developer-tooling workflow), not as a
  binary dependency or a required compatibility surface.
- Persistence of conversation history **beyond a single session** is
  out of scope for v1; conversations live only while HiveGUI is open.
