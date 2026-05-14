# Phase 1 Data Model: HiveClaw Agent & HiveGUI Client (Initial Scaffold)

**Branch**: `001-hiveclaw-hivegui` | **Date**: 2026-05-14 |
**Plan**: [plan.md](./plan.md) | **Spec**: [spec.md](./spec.md)

## Scope

v1 has **no persistent state**: HiveClaw is a stateless placeholder
service, and HiveGUI keeps conversation state in process memory for the
duration of a single session (per spec Assumption). The entities below
therefore describe in-memory Rust types in the `hivegui` crate; HiveClaw
exposes only the wire types in `contracts/openresponses-v1.md` and does
not own any of the entities here.

Each entity is presented as:

- **Fields** with Rust type sketches (final types live in
  `crates/hivegui/src/model/`).
- **Invariants** that the type / its module MUST enforce.
- **State transitions** where the entity has a lifecycle.

## Entity: `Conversation`

Module: `crates/hivegui/src/model/conversation.rs`

A chronological sequence of turns within a single HiveGUI session, plus
an at-most-one pending-turn slot (FR-008a).

**Fields**:

| Field | Type sketch | Notes |
|-------|-------------|-------|
| `id` | `ConversationId` (UUID v4) | Stable for the session. Used as the `tracing` correlation id for every outbound request. |
| `turns` | `Vec<ConversationTurn>` | Append-only during a session. Ordered by send time. |
| `pending` | `Option<PendingTurnId>` | At most one pending turn at a time (FR-008a). References a `ConversationTurn` whose `status == Pending` and lives inside `turns`. |
| `started_at` | `chrono::DateTime<Utc>` | Session start, populated on first `new()`. |

**Invariants** (enforced by the module's public API):

- I1. `pending.is_some()` ⇒ exactly one turn in `turns` has
  `status == Pending` and its id matches `pending`.
- I2. `pending.is_none()` ⇒ no turn in `turns` has `status == Pending`.
- I3. Turns are appended in ascending `created_at` order.
- I4. The `send_user_message` method MUST return `Err(BusyError)` when
  `pending.is_some()`; this is the in-model guard for FR-008a.

**Public API** (final names may differ slightly):

```rust
impl Conversation {
    pub fn new() -> Self;
    pub fn turns(&self) -> &[ConversationTurn];
    pub fn is_busy(&self) -> bool;             // pending.is_some()
    pub fn send_user_message(
        &mut self, content: UserMessage,
    ) -> Result<PendingTurnId, BusyError>;     // I4
    pub fn record_assistant_reply(
        &mut self, pending: PendingTurnId, reply: AssistantReply,
    );                                          // Pending → Delivered
    pub fn record_failure(
        &mut self, pending: PendingTurnId, error: TurnError,
    );                                          // Pending → Failed (retryable=true)
    pub fn retry(&mut self, failed: TurnId)
        -> Result<PendingTurnId, RetryError>;   // Failed → Pending (new id)
    pub fn dismiss_failure(&mut self, failed: TurnId);
}
```

## Entity: `ConversationTurn`

Module: `crates/hivegui/src/model/conversation.rs`

A single exchange element: either a user-authored message or an
assistant-authored reply.

**Fields**:

| Field | Type sketch | Notes |
|-------|-------------|-------|
| `id` | `TurnId` (UUID v4) | Stable per turn. |
| `author` | `Author` enum `{ User, Assistant }` | Speaker attribution required by FR-007. |
| `content` | `TurnContent` enum (see below) | Differs by author and stream mode. |
| `status` | `TurnStatus` enum `{ Pending, Delivered, Failed { retryable: bool } }` | See state machine below. |
| `created_at` | `chrono::DateTime<Utc>` | Set at construction. |
| `completed_at` | `Option<chrono::DateTime<Utc>>` | Set when leaving `Pending`. |
| `error` | `Option<TurnError>` | Present iff `status == Failed { .. }`. Human-readable Chinese message + machine-readable kind. |

**`TurnContent` variants**:

- `TurnContent::UserText { text: String }` — engineer's submitted text,
  already sanitised by `model::sanitize_user_input` per FR-011.
- `TurnContent::AssistantText { buffer: String }` — assistant reply.
  For streaming turns the buffer grows as `response.output_text.delta`
  events arrive (FR-008 incremental render); for sync turns it is set
  once when the JSON response lands.

**`TurnStatus` state machine** (FR-008 / FR-008a / Edge Case "becomes
unreachable mid-conversation"):

```
                                +--------------------+
                                |   record_assistant_|
                                |        reply       |
        send_user_message       v                    |
   (nothing) -----------> Pending --------------> Delivered
                            |  ^                     (terminal)
              record_failure|  | retry()
                            v  |
                          Failed { retryable: true }
                            |
                  dismiss_failure / new send
                            v
                       (terminal)
```

**Invariants**:

- T1. `Author::User` turns MAY go Pending → Delivered or Pending → Failed.
- T2. `Author::Assistant` turns are created in the `Delivered` state once
  the corresponding user turn's pending request has resolved successfully.
  They never enter `Pending` or `Failed`.
- T3. `status == Failed { retryable }` with `retryable == true` is the
  ONLY state in which the UI MAY show a "重试" affordance (FR-008a,
  spec Edge Cases). In v1 we set `retryable = true` for every transport /
  HTTP failure of a `Author::User` turn; we set `retryable = false`
  reserved for future use (e.g., schema-rejected requests) and do not
  emit it in v1.
- T4. `Conversation::send_user_message` MUST produce a User turn whose
  status starts at `Pending`. The assistant reply is appended as a
  SEPARATE `Author::Assistant` turn once the request resolves; the User
  turn moves to `Delivered` at the same moment.

## Entity: `ToolSeries`

Module: `crates/hivegui/src/model/tools.rs`

A named grouping of helper tools by data cadence. Exactly two series
exist in v1 and both ship with **zero** configured tools (FR-013,
FR-014).

**Fields**:

| Field | Type sketch | Notes |
|-------|-------------|-------|
| `kind` | `ToolSeriesKind` enum `{ DayPlusOne, HourPlusOne }` | Closed set in v1. |
| `display_name_zh` | `&'static str` | zh-CN label rendered in the home screen (`"Day+1 工具"`, `"Hour+1 工具"` — final copy in `ui/strings_zh.rs`). |
| `tools` | `Vec<HelperTool>` | Empty in v1 for both series. |

**Invariants**:

- S1. `ToolSeries::for_kind(ToolSeriesKind::DayPlusOne).tools.is_empty()`
  and the same for `HourPlusOne` hold in v1. The empty-state UI branch
  in `ui::home` MUST render whenever `tools.is_empty()`, including
  forward-compatibly when a series later gains tools — the same code
  path handles "no tools yet" and "loading failed produced an empty
  list" with distinct messages (spec Edge Cases).
- S2. `ToolSeriesKind` is `#[non_exhaustive]` only if/when a third
  series is on the roadmap; for v1 it is exhaustive.

## Entity: `HelperTool`

Module: `crates/hivegui/src/model/tools.rs`

A self-contained working surface inside HiveGUI belonging to exactly one
tool series.

**Fields**:

| Field | Type sketch | Notes |
|-------|-------------|-------|
| `id` | `HelperToolId` (string slug) | Stable across sessions; not used in v1 since no tools ship. |
| `series` | `ToolSeriesKind` | Back-reference. |
| `display_name_zh` | `String` | zh-CN display label. |
| `description_zh` | `String` | zh-CN one-liner shown in the series list. |
| `surface` | `HelperToolSurface` trait object | The actual gpui view a tool renders. v1 defines the trait but ships zero implementations. |

**Invariants**:

- H1. Every `HelperTool` instance MUST belong to exactly one `ToolSeries`
  (its `series` field is the source of truth; `ToolSeries::tools` is the
  index).
- H2. `HelperToolSurface` MUST own its own session-scoped state. The
  HiveGUI shell guarantees the trait object stays alive while the user
  navigates away to the conversation and back, so in-flight state is
  preserved (FR-010, SC-004).
- H3. v1 ships the trait but no implementors; `HelperTool` instances
  cannot be constructed by user-facing code in v1.

## Cross-entity rules

- C1. `Conversation`, `ToolSeries`, and any active `HelperTool` surfaces
  live as siblings under a single `HiveGuiApp` root (gpui `Model` /
  `Entity` instances). Switching between surfaces in the UI MUST NOT
  drop any of these (SC-004, FR-010).
- C2. No entity in this document is persisted to disk in v1. Logging
  emits structured records that REFERENCE these entities by id
  (`conversation_id`, `turn_id`) but MUST NOT serialise their full
  content (FR-012b — no full request/response payloads in logs).
- C3. Every outbound HTTP request HiveGUI makes to HiveClaw MUST carry
  the conversation's id as the correlation header / `tracing` span,
  satisfying constitution Principle VI's "correlation id propagated
  across service boundaries".

## Out of scope for v1 (documented for the next feature)

- Persisted conversation history (would land in SQLite; needs schema +
  migration design).
- Tool-local persistent state (would land in sled keyed by
  `HelperToolId`).
- Multi-conversation switching inside one HiveGUI window.
- User identity / per-user namespacing of the above.
