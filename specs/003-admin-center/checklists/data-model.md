# Data Model Requirements Quality Checklist: 管理中心

**Purpose**: Validate whether the data-model requirements in `data-model.md` (cross-referenced with `spec.md`) are complete, clear, consistent, and measurable — as an author self-check before sharing the spec for review. Items also flag data-model behaviors the implementation already exhibits that the spec never specified.
**Created**: 2026-05-26
**Feature**: [specs/003-admin-center/spec.md](file:///home/developer/agent/hive-claw/specs/003-admin-center/spec.md) · [data-model.md](file:///home/developer/agent/hive-claw/specs/003-admin-center/data-model.md)
**Scope**: Spec + implemented reality (data-model gaps surfaced by what the code already does)
**Audience / Depth**: Author self-check (lightweight) — obvious gaps and ambiguities only

---

## Requirement Completeness — Entities & Fields

- [ ] CHK001 Are `created_at` / `updated_at` storage and display semantics specified — server-generated only, immutable on update, timezone in storage vs API vs UI? [Completeness, Spec §Admin]
- [ ] CHK002 Is a soft-delete vs hard-delete decision documented for `Admin` (spec.md says delete is allowed for non-Super, but nothing in data-model.md indicates which)? [Gap, Spec §FR-009]
- [ ] CHK003 Are referential-integrity requirements specified for `LoginRecord.admin_id` when an admin is deleted (cascade is in the DDL but the requirement itself isn't stated)? [Gap, Spec §LoginRecord]
- [ ] CHK004 Is the lifecycle of `last_login_at` defined (set only on success, never decremented, behavior when an admin is re-enabled after long inactivity)? [Completeness, Spec §FR-003]
- [ ] CHK005 Are requirements defined for tracking which admin created/updated another admin (audit trail beyond `LoginRecord`)? [Gap]
- [ ] CHK006 Are the data-model requirements for "online managers" (used by FR-014) defined — does it derive from `last_login_at` window, an explicit session table, or Redis? [Gap, Spec §FR-014]
- [ ] CHK007 Is a per-IP / per-phone failed-login counter modeled as a first-class entity, or only as ephemeral Redis state? Where is the requirement stated? [Gap, Spec §FR-017]

## Requirement Clarity — Types & Encodings

- [ ] CHK008 Is the `phone` storage type definitively specified — `VARCHAR(11)` numeric-only, allowing leading-zero preservation and excluding `+86` prefixes? [Clarity, Spec §Admin]
- [ ] CHK009 Is the `password_hash` length of exactly 60 characters justified by bcrypt's output format, and what happens if a future algorithm produces a longer hash? [Clarity, Gap]
- [ ] CHK010 Is the `ip_address` column expected to store the client IP, the first X-Forwarded-For hop, or the proxy IP — and is normalization (`::ffff:`-mapped IPv4) defined? [Ambiguity, Spec §LoginRecord]
- [ ] CHK011 Are the `role` and `status` numeric encodings (1/2/3, 0/1) stated as a stable contract for external consumers (frontend, exports), or as an internal-only mapping? [Clarity, Spec §Admin]
- [ ] CHK012 Is `failure_reason` defined as a closed enum (`WRONG_PASSWORD | ACCOUNT_DISABLED | ACCOUNT_LOCKED | OTHER`) or a free-form string of up to 50 chars? The DDL allows the latter; the validation rule implies the former. [Conflict, Spec §LoginRecord]

## Requirement Consistency — Schema vs Entity vs Spec

- [ ] CHK013 Is the `nickname` length consistent? Entity says "2–20 字符", DDL says `VARCHAR(20)`, spec assumptions don't say. Is the **2-char minimum** an enforced data-layer constraint or only application-layer? [Consistency, Spec §Admin]
- [ ] CHK014 Are the migration filenames consistent? Spec describes `V001__create_admins_table.sql` / `V002__create_login_records_table.sql` / `V003__seed_super_admin.sql`, but the implementation runs migrations inline from `crates/hiveweb-admin/src/bin/migrate.rs`. Which is the source of truth? [Conflict, Spec §Migrations]
- [ ] CHK015 Are the `RolePermission` entries consistent with the spec's role descriptions? Spec §US3 says System admin sees the delete button "禁用状态或不可见", but `RolePermission.System` does not include `admin-delete` at all — clarify whether System has the delete menu hidden vs disabled. [Consistency, Spec §US3, §RolePermission]
- [ ] CHK016 Are the `updated_at` semantics consistent between the entity description ("最后更新时间") and the DDL (`ON UPDATE CURRENT_TIMESTAMP`) — does `last_login_at` change bump `updated_at`? [Consistency, Spec §Admin]
- [ ] CHK017 Are uniqueness requirements consistent — entity says "phone 唯一索引", DDL uses `UNIQUE` + a non-unique `INDEX idx_phone (phone)`. Is the redundant secondary index intentional? [Consistency, Spec §Admin]
- [ ] CHK018 Are the validation rules consistent with spec assumptions? Spec assumes "密码长度 6-20 位, 支持字母和数字组合"; `validate_password()` checks length only. Is the alphanumeric requirement intentional or dropped? [Conflict, Spec §Assumptions]

## Acceptance Criteria Quality — Constraints

- [ ] CHK019 Is "至少保留一个启用的 Super 管理员" expressed as a verifiable constraint (DB-level check, application-level guard, or both), and what does the system do if a race condition produces zero? [Measurability, Spec §FR-012]
- [ ] CHK020 Are the constraints on `created_at` ("creation time immutable") enforced at the data layer (no DDL-level prevention exists today) or only at the service layer? [Measurability, Spec §Admin Validation Rules]
- [ ] CHK021 Is the data-model coverage of "phone uniqueness" measurable across concurrent inserts (the DB enforces it, but is the requirement for the user-facing error explicitly traceable to the unique-constraint violation)? [Measurability, Spec §FR-011]

## Scenario Coverage — Data lifecycles

- [ ] CHK022 Are data requirements specified for the **initial-bootstrap** scenario (V003 mentions a seeded super admin but says "by script"; is the seeded admin's password reset requirement specified)? [Coverage, Gap]
- [ ] CHK023 Are data requirements specified for the **disable-then-re-enable** flow — must any field be reset (failed-login counter, `last_login_at`)? [Coverage, Gap]
- [ ] CHK024 Are data requirements specified for the **delete** flow — are login records retained (cascade currently deletes them), or must they be preserved for audit? [Conflict, Spec §LoginRecord vs FR-009]
- [ ] CHK025 Are data requirements specified for the **password-change** flow — is the old hash retained anywhere, are concurrent sessions invalidated? [Gap]
- [ ] CHK026 Are data requirements specified for **token revocation** — does logout / disable need to persist anything (denylist, version counter on Admin), or is JWT statelessly expired? [Gap, Spec §FR-019]

## Edge Case Coverage — Data

- [ ] CHK027 Are requirements defined for an admin whose `last_login_at` is NULL (never logged in) — is this displayable, sortable, and how does the dashboard "online admins" count handle it? [Edge Case, Spec §Admin]
- [ ] CHK028 Are requirements defined for the maximum number of `LoginRecord` rows per admin (retention, purging, partitioning) given SC-005 (100+ admins) and high login volume? [Gap, Spec §SC-005]
- [ ] CHK029 Are requirements defined for clock skew between the application server and the database (DDL uses `CURRENT_TIMESTAMP` while the Rust struct uses `chrono::Utc`)? [Edge Case, Gap]
- [ ] CHK030 Are requirements defined for very long IPv6 addresses (45 chars covers most, but scoped/zone IDs can exceed it)? [Edge Case, Spec §LoginRecord]

## Non-Functional — Data persistence

- [ ] CHK031 Are indexing requirements complete given the dashboard query "recent 10 logins joined to admin nickname" — should `idx_login_at` be descending, and is a covering index needed? [Completeness, Spec §FR-015]
- [ ] CHK032 Are backup/retention requirements specified for the `admins` and `login_records` tables (RPO/RTO, retention period for audit records)? [Gap]
- [ ] CHK033 Are encryption-at-rest requirements specified for the database (passwords are hashed, but phone numbers are PII)? [Gap]
- [ ] CHK034 Are observability requirements specified for slow queries on these tables (logging threshold, alerting)? [Gap]

## Dependencies & Assumptions

- [ ] CHK035 Is the assumption "MySQL 8.0+ InnoDB" called out as a hard dependency for the chosen DDL features (`ON UPDATE CURRENT_TIMESTAMP`, `utf8mb4_unicode_ci`, foreign keys)? [Assumption, Spec §Assumptions]
- [ ] CHK036 Is the connection-pool config (max=20, min=5, connect_timeout=30s, idle=600s) stated as a requirement, a tuned default, or an example? [Clarity, Spec §MySQL Configuration]
- [ ] CHK037 Is bcrypt cost=12 stated as a fixed requirement or a tunable parameter; is upgrading the cost in the future a planned migration? [Clarity, Spec §Admin Constraints]

## Implementation-vs-Spec Gaps (informed by current code)

- [ ] CHK038 The implementation runs schema creation from `crates/hiveweb-admin/src/bin/migrate.rs` rather than the versioned `V00X__*.sql` files described in `data-model.md` §Migrations. Should the spec be updated to match (or vice versa)? [Gap, Spec §Migrations]
- [ ] CHK039 The code stores failed-login counters in Redis (via `services/auth.rs`); `data-model.md` does not list Redis as part of the data model. Should Redis-state requirements (key schema, TTL, eviction policy) be documented here or in a separate cache spec? [Gap]
- [ ] CHK040 The `Admin` Rust struct includes `updated_at`, but it never appears in `AdminResponse`. Is the API-level visibility of `updated_at` a deliberate omission documented somewhere? [Gap, Spec §Admin / DTOs]
- [ ] CHK041 The `create_super_admin` binary inserts directly into `admins`. Is there a data-model requirement that this insertion bypass the normal validation (e.g. skip the "must have inviter" rule if one existed)? [Gap]
- [ ] CHK042 The implementation does not write a `LoginRecord` for every attempt (per spec the table is for "登录历史"). Is the requirement explicit about whether **failed** attempts are recorded, or only successful ones? `failure_reason` exists, but the obligation to write a row on failure isn't stated. [Ambiguity, Spec §LoginRecord]

## Ambiguities & Traceability

- [ ] CHK043 Is there a stable ID scheme linking data-model rules (e.g. "phone unique", "last super admin guard") to FR/SC items, so a change to the schema can be traced back to the originating requirement? [Traceability]
- [ ] CHK044 Are open data-model questions (the `[Gap]` items above) tracked somewhere — a clarifications log, follow-up issues, or an explicit "deferred to v2" list — so reviewers know they were considered? [Traceability]

---

## Notes

- Check items off as you resolve them in `data-model.md` / `spec.md` (or document a deliberate "out of scope" decision next to the item).
- This checklist tests the **requirements**, not the implementation. An item is "passing" when the spec answers it — not when the database accepts the data.
- Items flagged from the current code (CHK038–CHK042) should be back-filled into the spec so future contributors don't have to read migrations and service code to learn intent.
