<!--
SYNC IMPACT REPORT
==================
Version change: 1.4.0 → 1.5.0
Rationale: MINOR bump. Adds a single-developer repository clause to clarify how
the Security Requirements "second approver" rule and the Development Workflow
"two approving reviews" rule are satisfied for repositories with a single
active maintainer. No principle is removed or redefined; the new clause is
explicitly called out so it cannot be conflated with a waiver of the security
review itself.

Modified sections:
  - Security Requirements — adds the single-developer repository clause.
  - Development Workflow & Quality Gates — adds the matching single-developer
    rule for the two-approving-review requirement, mirroring the Security
    clause so both review gates share one definition of "single-developer".
  - Governance — amendment procedure is unchanged; the new clause is a
    clarification of how existing review requirements apply, not a new
    approval gate.

Added sections: none as standalone; the single-developer repository clause
is appended to two existing sections.

Removed sections: none.

Templates and runtime guidance updated:
  - ✅ .specify/templates/plan-template.md — Constitution Check now references
        the single-developer clause and the equivalent review/approver gate.
  - ✅ .specify/templates/spec-template.md — explicit reference retained.
  - ✅ .specify/templates/tasks-template.md — explicit reference retained.
  - ✅ CLAUDE.md and AGENTS.md — must cite the new clause.
  - ✅ docs/quickstart.md — quickstart inherits the same rule.
  - ✅ specs/011-hivegui-standalone-mode/{plan,tasks,checklists/security.md,
        checklists/implementation-review.md} — affected feature alignment and
        implementation gates (T025R 6 boundaries).

Deferred follow-ups: none for this amendment.
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

**Single-developer repository clause (2026-07-30, v1.5.0)**: For repositories
with a single active maintainer (i.e. no second human reviewer is available
in the maintainer set, as recorded in the project's `CODEOWNERS` or
equivalent), the dedicated security review and the second-approver
requirements above are satisfied by the **single maintainer performing both
roles** under the following non-waivable conditions:

1. The dedicated `/security-review` (or equivalent) workflow MUST still run
   to completion and the conclusion, evidence links, and any open findings
   MUST be recorded in the PR description and in the relevant security
   checklist (e.g. `checklists/security.md` for T025R boundaries).
2. The single maintainer MUST record a self-attestation in the PR
   description with: their handle, the date, the security-review
   conclusion, the exact list of clauses that were re-checked, and an
   explicit acknowledgement that they acted as both implementer and
   approver because the repository has no other active maintainer.
3. Every security-review conclusion and self-attestation MUST be
   reproduced verbatim in the relevant checklist row. The row is not
   "signed" until both the workflow output and the self-attestation are
   present and dated.
4. If a second maintainer joins the project later, the regular
   "independent security reviewer + second approver" requirements
   immediately resume for new PRs; historical self-attestations are not
   retroactively invalidated but are explicitly marked as "single-developer
   repository clause" so reviewers can see which sign-offs pre-date the
   second maintainer joining.

This clause does NOT waive: the dedicated security review, the
implementation of the security controls, the doc hard gate, the dependency
advisory ban, secret scanning, or any other constitutional article. It only
removes the structural requirement that the second approver be a different
human being.

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

- **Language (backend)**: Rust (stable channel). The MSRV (Minimum Supported
  Rust Version) MUST be pinned in `rust-toolchain.toml` and bumped only in a
  dedicated PR. No other application languages may be introduced for backend
  feature work; small build / dev tooling scripts in shell or Python are
  permitted at the workspace root.
- **Language (frontend)**: TypeScript (strict mode) with React as the UI
  framework. The TypeScript version SHOULD be pinned in `package.json` or
  equivalent. JavaScript-only code is PROHIBITED for new frontend work.
- **Project layout**: a single Cargo workspace at the repository root for
  Rust backend crates. Each deployable unit (HiveClaw, HiveGUI) is its own
  workspace member crate. Frontend web projects live as separate npm packages
  (e.g., `web/`, `packages/*`) outside the Cargo workspace. A shared library
  workspace member (e.g., `hive-shared`) is permitted only when Principle V's
  "second concrete caller" test is satisfied.
- **Lint & format (Rust)**: `cargo fmt` and `cargo clippy` (with
  `-D warnings` in CI) are the project's enforced lint/format tools per
  Principle I.
- **Lint & format (TypeScript)**: ESLint and Prettier (or Biome) are the
  enforced tools; configuration MUST be committed to the repository.
- **Desktop GUI (HiveGUI)**: **gpui** is the canonical desktop UI framework.
  HiveGUI MUST be built on gpui; no second desktop UI framework may be
  introduced. Per Principle V, gpui primitives MUST be used directly —
  wrappers are only permitted when they encode a non-trivial invariant.
- **Web Frontend**: **React** (with TypeScript) is the canonical web UI
  framework. Per Principle V, React primitives (components, hooks, context)
  MUST be used directly — wrappers are only permitted when they encode a
  non-trivial invariant. State management SHOULD use React's built-in hooks
  (useState, useContext, useReducer) before introducing external state
  management libraries.
- **HTTP / API (HiveClaw and any future service)**: **axum** is the canonical
  HTTP server framework, running on the Tokio runtime. Request handlers MUST
  use `axum`'s extractors and response types directly; no parallel HTTP
  framework may be introduced.
- **Database**: **MySQL 8.0+** (InnoDB engine) is the canonical relational
  database for server production data and all products outside an explicitly
  named runtime profile. Schema changes MUST use versioned, idempotent
  migrations; access MUST use SQLx (with compile-time SQL verification) or a
  maintained Rust MySQL client. Connection pooling is MANDATORY (recommended:
  SQLx pool, max_connections tuned per workload).
- **Object Storage**: **Rustfs** (S3-compatible) is the canonical server object
  storage for file uploads, backups, and static assets. The Rust `aws-sdk-s3`
  crate or `object_store` crate SHOULD be used for S3 interoperability.
  Direct filesystem storage is PROHIBITED for new features unless explicitly
  authorized by the HiveGUI desktop-local runtime profile below or justified
  for local development only.
- **Cache**: **Redis 7+** is the canonical cache and session store for server
  production workloads. Use the `redis` or `bb8-redis` crate for connection
  pooling. Caching strategies (cache-aside, write-through, etc.) MUST be
  documented in the relevant plan. Session data MUST expire (TTL required).
- **Desktop-local runtime profile (HiveGUI)**: HiveGUI is an independent local
  Agent, not a client of the cloud-hosted HiveWeb Agent. HiveGUI MUST NOT call
  HiveWeb APIs, read HiveWeb connection settings as a runtime prerequisite, or
  fall back to HiveWeb when local execution fails. Compile-time code, models,
  ABI contracts, and test fixtures MAY be shared between the products. Within
  HiveGUI only, production data MAY use embedded SQLite instead of MySQL;
  persistent sessions MAY use encrypted SQLite and bounded in-process caches
  instead of Redis; and Plugin WASM, logs, and user-selected backups MAY use a
  managed local filesystem instead of Rustfs/S3. This profile requires:
  versioned transactional migrations; SQLite integrity and foreign-key checks;
  owner-only secret material; path containment and symlink-escape prevention;
  size and SHA-256 verification; staging, fsync, and atomic rename for managed
  artifacts; explicit retention/expiry rules; and bounded caches with a
  documented key, invalidation strategy, and capacity. These permissions do
  not apply to HiveWeb, whose production profile remains MySQL, Redis, and
  Rustfs/S3.
- **Async runtime**: **Tokio** (implied by axum and the broader Rust
  async ecosystem). A second async runtime MUST NOT be introduced in v1.
- **Testing (Rust)**: `cargo test` for unit and integration tests; crate-level
  contract tests live alongside the crate they test. Tests MUST run in CI
  as part of the standard quality gates.
- **Testing (TypeScript/React)**: Vitest or Jest for unit tests; Testing
  Library for React component tests. Tests MUST run in CI as part of the
  standard quality gates.

**Deviation procedure**: a feature plan that requires a different language,
GUI/HTTP framework, or datastore MUST record the deviation in its Plan's
Complexity Tracking section with: (a) the specific need that the canonical
stack cannot satisfy, (b) the alternative chosen, (c) the simpler approach
considered and rejected, and (d) the maintenance / review-expertise impact.
The amendment procedure under Governance applies if the deviation is
intended to become permanent. A choice expressly authorized by a named
Technology Stack profile is compliant use of that profile, not a deviation;
the plan MUST still identify the profile and prove its mandatory safeguards.

**Rationale**: A modern, production-ready stack: Rust+gpui for desktop,
TypeScript+React for web, MySQL/Redis/Rustfs for cloud services, and a bounded,
secure local-storage profile for an independent desktop Agent. Each choice
balances performance, deployment reality, and maintainability while keeping
product boundaries explicit.

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
  - **Single-developer repository clause**: for repositories with a single
    active maintainer, the "minimum of two approving reviews" requirement
    is satisfied by the single maintainer self-approving under the same
    non-waivable conditions defined in the Security Requirements
    single-developer repository clause. The dedicated security review, the
    implementation of all constitutional controls, secret scanning,
    dependency advisory bans, and the doc hard gate are NOT waived.
- **CI gates** (all MUST pass before merge):
  1. Linting and formatting (Rust and TypeScript).
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
  `AGENTS.md`, `CLAUDE.md`, `docs/quickstart.md`, and any future equivalent.
  Those files MUST cite, not contradict, this constitution.

**Version**: 1.5.0 | **Ratified**: 2026-05-14 | **Last Amended**: 2026-07-30
