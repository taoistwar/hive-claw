<!--
SYNC IMPACT REPORT
==================
Version change: TEMPLATE (unratified) → 1.0.0
Rationale: Initial ratification. All placeholder tokens replaced with concrete
principles, standards, and governance. MAJOR version chosen because this is the
first ratified version establishing the baseline contract.

Modified principles (renames from template placeholders):
  - [PRINCIPLE_1_NAME] → I. Code Quality & Maintainability
  - [PRINCIPLE_2_NAME] → II. Test-First Development (NON-NEGOTIABLE)
  - [PRINCIPLE_3_NAME] → III. User Experience Consistency
  - [PRINCIPLE_4_NAME] → IV. Performance & Efficiency
  - [PRINCIPLE_5_NAME] → V. Simplicity & YAGNI
  - (added) VI. Observability & Structured Logging

Added sections:
  - Security Requirements (mandatory standards for input handling, secrets, auth)
  - Performance Standards (response time, query, indexing rules)
  - Development Workflow & Quality Gates (PR review, approvals, CI gates)
  - Governance (amendment procedure, versioning, compliance review)

Removed sections: none (template had only placeholders).

Templates requiring updates:
  - ✅ .specify/templates/plan-template.md — "Constitution Check" section is
        intentionally generic (`[Gates determined based on constitution file]`)
        and stays compatible. Technical Context already lists the <200ms p95
        constraint as an example, matching Principle IV. No edit required.
  - ✅ .specify/templates/spec-template.md — no constitution-specific section
        names need updating; spec template is principle-agnostic.
  - ✅ .specify/templates/tasks-template.md — already includes logging and
        security hardening task categories; consistent with Principles II/VI
        and Security Requirements. No edit required.
  - ✅ .specify/templates/checklist-template.md — generic, principle-agnostic.
  - ⚠ README.md — currently empty; project overview should be authored
        separately. Not blocking constitutional adoption.
  - ⚠ docs/quickstart.md — does not exist yet; create when first feature
        introduces runtime guidance needs.

Follow-up TODOs:
  - None. Ratification date set to today (initial adoption).
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

**Version**: 1.0.0 | **Ratified**: 2026-05-14 | **Last Amended**: 2026-05-14
