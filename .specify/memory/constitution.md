<!--
SYNC IMPACT REPORT
==================
Version change: 1.0.0 → 1.1.0
Rationale: MINOR bump. Adds a new mandatory "Technology Stack" section that
locks the canonical implementation stack (Rust + gpui + axum + sled + SQLite)
for all v1 work. No existing principle is removed or redefined; existing
compliance gates remain valid.

Modified principles: none (all six core principles unchanged).

Added sections:
  - Technology Stack (canonical languages, frameworks, datastores, deviation
    procedure)

Removed sections: none.

Templates requiring updates:
  - ✅ .specify/templates/plan-template.md — Technical Context placeholders
        (Language/Version, Primary Dependencies, Storage, Testing) now have
        defined defaults from this section. Plans MAY still record version
        pins but MUST NOT pick a different language or core framework
        without Complexity Tracking.
  - ✅ .specify/templates/spec-template.md — unaffected (spec template is
        tech-agnostic by design).
  - ✅ .specify/templates/tasks-template.md — unaffected (task categories
        are tech-agnostic).
  - ✅ .specify/templates/checklist-template.md — unaffected.
  - ⚠ README.md — still empty; should now mention the Rust toolchain as
        the prerequisite for contributors. Not blocking.
  - ⚠ docs/quickstart.md — does not exist yet; will be authored alongside
        the first implementation plan.

Prior version 1.0.0 history retained below for context.

----
Previous: TEMPLATE (unratified) → 1.0.0 (initial ratification, 2026-05-14)
  Added principles I–VI; added Security Requirements, Performance Standards,
  Development Workflow & Quality Gates, Governance.
-->

# hive-claw Constitution

## Core Principles

### I. Code Quality & Maintainability

All code MUST be readable, consistent, and self-explanatory through naming and
structure rather than comments. Every change MUST pass project linting and
formatting checks before merge. Public interfaces (functions, modules, APIs)
MUST be documented with their contract: inputs, outputs, error modes. Dead
code, commented-out code, and TODOs without an owner or tracked issue MUST NOT
be merged. Cyclomatic complexity in a single function SHOULD remain low; when
a function exceeds reasonable bounds, it MUST be decomposed before merge.

**Rationale**: Code is read far more often than written. Enforcing a high
quality bar at merge time prevents the slow accretion of debt that makes
codebases unmaintainable.

### II. Test-First Development (NON-NEGOTIABLE)

Strict TDD is mandatory for all feature work and bug fixes:

1. Tests MUST be written before implementation.
2. Tests MUST be reviewed and approved by the user (or the designated
   reviewer) before any production code is written.
3. Tests MUST be observed to fail (Red) before implementation begins.
4. Implementation MUST be the minimum code required to make tests pass
   (Green), followed by Refactor.

Contract tests MUST exist for every external interface (HTTP endpoint, CLI
command, public library API). Integration tests MUST exist for every
inter-service boundary and shared schema. Unit tests SHOULD cover the
non-trivial branches of internal logic. A PR that adds production code without
corresponding failing-then-passing tests MUST be rejected.

**Rationale**: Writing tests first forces clearer specifications, prevents
over-engineering, and produces a regression safety net that compounds in
value over the lifetime of the codebase.

### III. User Experience Consistency

User-facing surfaces (CLI, HTTP API, UI) MUST present consistent behaviour
across the project:

- Naming, error formats, exit codes, status codes, and pagination MUST follow
  one documented convention per surface type.
- Error messages MUST be actionable: they MUST identify the cause and, where
  possible, suggest the next step.
- Breaking changes to a user-facing contract MUST follow the Versioning rules
  in Governance and MUST ship with a migration note.
- Accessibility (where a UI exists): keyboard navigation, semantic markup,
  and sufficient colour contrast MUST be verified before merge.

**Rationale**: Consistency reduces the cognitive load on users and integrators
and is the cheapest form of usability we can ship.

### IV. Performance & Efficiency

Performance is a feature, not an afterthought. The following are
**non-negotiable budgets** unless an explicit, documented exception is
recorded in the Complexity Tracking section of the relevant plan:

- API endpoints: p95 latency under load MUST be < 200ms.
- Database access: every query in the hot path MUST use an index; full-table
  scans on non-trivial tables MUST be justified in writing.
- N+1 query patterns are PROHIBITED; batch, join, or pre-fetch instead.
- Caches and memoisation MUST have a documented invalidation strategy.

Performance-relevant changes MUST include a measurement (benchmark, load test
result, or production metric link) demonstrating the budget is met.

**Rationale**: Performance regressions are far more expensive to detect and
fix after release than to prevent at review time. Hard, measurable budgets
make the bar enforceable.

### V. Simplicity & YAGNI

Build the smallest thing that works:

- Initial implementation of any feature MUST use **at most three projects /
  deployable units** (e.g., one service + one client + one shared library).
  Adding a fourth requires Complexity Tracking justification.
- Future-proofing, configuration knobs without a current consumer, and
  speculative abstractions are PROHIBITED. Add them when the second concrete
  caller arrives, not before.
- Framework features MUST be used directly. Wrapping a framework primitive
  in a project-specific abstraction is only permitted when the wrapper
  encodes a non-trivial invariant that the raw primitive does not.
- Three similar lines are preferable to a premature abstraction.

**Rationale**: Every abstraction is a tax on future readers. Deferring them
until they pay rent keeps the codebase honest.

### VI. Observability & Structured Logging

Every service and long-running process MUST emit structured logs (JSON or
equivalent key-value format) suitable for machine parsing. Logs MUST include:

- A correlation / request ID propagated across service boundaries.
- The operation name, outcome (success / error class), and duration.
- No secrets, credentials, PII, or full request/response bodies in plain text.

Errors MUST be logged at the boundary where they are handled, exactly once,
with sufficient context to diagnose without re-running. Metrics and traces
SHOULD complement logs for hot paths; logs alone are not sufficient for
high-volume systems.

**Rationale**: Production incidents are won or lost on the quality of
telemetry available at 3 a.m. Structured, consistent observability is the
prerequisite for everything else.

## Security Requirements

These standards apply to every feature and MUST be enforced at code review:

- **Input validation**: All input crossing a trust boundary (HTTP request,
  CLI argument, message queue payload, file upload) MUST be validated and
  sanitised before use. Validation MUST happen at the boundary, not deep
  inside business logic.
- **Output encoding**: Data rendered into HTML, SQL, shell commands, or any
  other interpreter MUST use context-appropriate encoding or parameterised
  APIs. String concatenation into these contexts is PROHIBITED.
- **Secrets**: Credentials, API keys, tokens, and private keys MUST NOT be
  committed to the repository. Use a secrets manager or environment-injected
  configuration. CI MUST scan for accidentally committed secrets.
- **Authentication & authorisation**: Any feature that adds, modifies, or
  touches authentication, authorisation, session handling, or access control
  MUST receive a dedicated security review (the `/security-review` workflow
  or equivalent) before merge. A second approver with security context is
  required on the PR.
- **Dependencies**: New third-party dependencies MUST be vetted for
  maintenance status and known CVEs. Vulnerable versions MUST be upgraded
  within the SLA defined by the project's security policy.

## Performance Standards

Concrete, enforceable targets that elaborate Principle IV:

- **API p95 latency**: < 200ms under representative load. p99 SHOULD be
  documented per endpoint.
- **Database**: every production query MUST be EXPLAIN-verified to use an
  index on its filter and join columns. Migrations that add a column queried
  in the hot path MUST also add the supporting index.
- **N+1 detection**: ORMs and data-access layers MUST be configured to log
  or fail on detected N+1 patterns in test environments.
- **Payload size**: API responses SHOULD support pagination when the
  collection can exceed 100 items; unbounded list endpoints are PROHIBITED.
- **Regression gate**: a change that causes a > 10% regression on any
  tracked performance benchmark MUST NOT merge without explicit sign-off
  and a recorded justification.

## Technology Stack

The following stack is **canonical** for all v1 implementation work in this
project. It exists to keep cognitive load, build infrastructure, and review
expertise concentrated — not to discourage learning. Deviation requires an
explicit Complexity Tracking entry in the relevant plan.

- **Language**: Rust (stable channel). The MSRV (Minimum Supported Rust
  Version) MUST be pinned in `rust-toolchain.toml` and bumped only in a
  dedicated PR. No other application languages may be introduced for v1
  feature work; small build / dev tooling scripts in shell or Python are
  permitted at the workspace root.
- **Project layout**: a single Cargo workspace at the repository root. Each
  deployable unit (HiveClaw, HiveGUI) is its own workspace member crate.
  A third workspace member (e.g., `hive-shared`) is permitted only when
  Principle V's "second concrete caller" test is satisfied.
- **Lint & format**: `cargo fmt` and `cargo clippy` (with `-D warnings` in CI)
  are the project's enforced lint/format tools per Principle I.
- **Desktop GUI (HiveGUI)**: **gpui** is the canonical UI framework. HiveGUI
  MUST be built on gpui; no second UI framework may be introduced. Per
  Principle V, gpui primitives MUST be used directly — wrappers are only
  permitted when they encode a non-trivial invariant.
- **HTTP / API (HiveClaw and any future service)**: **axum** is the canonical
  HTTP server framework, running on the Tokio runtime. Request handlers MUST
  use `axum`'s extractors and response types directly; no parallel HTTP
  framework may be introduced.
- **Embedded KV store**: **sled** is the canonical embedded key-value store
  for local on-disk state that does not require relational queries (e.g.,
  caches, simple session-scoped state, tool-local stores).
- **Embedded relational store**: **SQLite** is the canonical embedded
  relational store for any structured, queryable, or schema-evolving data.
  Access MUST use a maintained Rust SQLite binding; schema changes MUST
  be delivered via versioned, idempotent migrations.
- **Async runtime**: **Tokio** (implied by axum and the broader Rust
  async ecosystem). A second async runtime MUST NOT be introduced in v1.
- **Testing**: `cargo test` for unit and integration tests; crate-level
  contract tests live alongside the crate they test. Tests MUST run in CI
  as part of the standard quality gates (see Development Workflow).

**Deviation procedure**: a feature plan that requires a different language,
GUI/HTTP framework, or datastore MUST record the deviation in its Plan's
Complexity Tracking section with: (a) the specific need that the canonical
stack cannot satisfy, (b) the alternative chosen, (c) the simpler approach
considered and rejected, and (d) the maintenance / review-expertise impact.
The amendment procedure under Governance applies if the deviation is
intended to become permanent.

**Rationale**: A single small, modern Rust stack matches the project's
single-user local desktop posture (HiveGUI) and its embedded service
posture (HiveClaw), satisfies Principle V (≤3 projects, no premature
abstraction) and Principle IV (Rust's low overhead makes the < 200ms API
budget trivial outside of agent reasoning paths), and minimises the
review and tooling surface every contributor must master.

## Development Workflow & Quality Gates

- **Branching**: feature work happens on feature branches. Direct commits
  to the main branch are PROHIBITED.
- **Pull requests**: every change MUST land via a PR.
  - All PRs require at least one approving review.
  - Changes to **core modules** (defined per project in `CODEOWNERS` or
    equivalent) require a **minimum of two approving reviews**, at least
    one from a code owner.
  - Auth, security, or cryptography changes additionally require the
    Security Requirements review described above.
- **CI gates** (all MUST pass before merge):
  1. Linting and formatting.
  2. Type checking (where applicable).
  3. Unit, integration, and contract test suites.
  4. Secret scanning.
  5. Dependency vulnerability scan.
- **Merge hygiene**: commits MUST be logically coherent; squash on merge is
  preferred unless history is intentionally meaningful. PR descriptions MUST
  explain the *why*, link to the spec, and note any constitutional exceptions
  taken (with Complexity Tracking entries).
- **Post-merge**: failing main-branch builds MUST be fixed or reverted within
  one business day; no new work merges on top of a broken main.

## Governance

- **Supremacy**: This constitution supersedes ad-hoc conventions. Where a
  team practice conflicts with the constitution, the constitution wins until
  amended.
- **Amendment procedure**: amendments are proposed via PR against
  `.specify/memory/constitution.md`. The PR MUST include:
  1. A Sync Impact Report (as a leading HTML comment) describing version
     change, modified / added / removed sections, and template impact.
  2. Updates to any dependent templates and runtime guidance touched by
     the amendment.
  3. Approvals from at least two maintainers; security or workflow changes
     additionally require a maintainer with that domain.
- **Versioning policy** (semantic):
  - **MAJOR**: backward-incompatible governance change, principle removal,
    or redefinition that invalidates prior compliance.
  - **MINOR**: a new principle or materially expanded section.
  - **PATCH**: clarifications, wording, typo fixes, or non-semantic
    refinements.
- **Compliance review**: every PR review MUST verify the change is
  constitution-compliant. Plans (`/speckit-plan`) MUST run the Constitution
  Check gate before Phase 0 and re-check after Phase 1. Justified violations
  MUST be recorded in the plan's Complexity Tracking section with a simpler
  alternative considered.
- **Runtime guidance**: agent and contributor runtime instructions live in
  `CLAUDE.md` and any future `docs/quickstart.md`. Those files MUST cite,
  not contradict, this constitution.

**Version**: 1.1.0 | **Ratified**: 2026-05-14 | **Last Amended**: 2026-05-14
