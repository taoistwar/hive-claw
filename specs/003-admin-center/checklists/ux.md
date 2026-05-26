# UX Requirements Quality Checklist: 管理中心

**Purpose**: Validate whether UX requirements in `spec.md` are complete, clear, consistent, and measurable — as an author self-check before sharing the spec for review. Items also flag UX behaviors the implementation already exhibits that the spec never specified.
**Created**: 2026-05-26
**Feature**: [specs/003-admin-center/spec.md](file:///home/developer/agent/hive-claw/specs/003-admin-center/spec.md)
**Scope**: Spec + implemented reality (UX requirement gaps surfaced by what the code already does)
**Audience / Depth**: Author self-check (lightweight) — focus on obvious gaps and ambiguities

---

## Requirement Completeness — UX

- [ ] CHK001 Are form-field validation requirements (timing, error placement, inline vs summary) defined for the login form? [Gap, Spec §FR-001]
- [ ] CHK002 Are field requirements defined for the "添加管理员" form (which fields are required, optional, defaultable, and in what order)? [Completeness, Spec §FR-006]
- [ ] CHK003 Are pagination UX requirements defined (page size, page-size selector, total-count display, jump-to-page)? [Gap, Spec §FR-010 / Edge Cases]
- [ ] CHK004 Are loading-state requirements specified for admin list, dashboard cards, and recent-logins table? [Gap]
- [ ] CHK005 Are empty-state requirements specified (no admins beyond self, no login records, dashboard with zero today-logins)? [Gap]
- [ ] CHK006 Are network/server-error UI requirements defined (offline, 5xx, timeout) beyond the login-specific error in §FR-001? [Gap]
- [ ] CHK007 Are confirmation-dialog requirements specified (wording, destructive styling, default focus, cancel behavior) for delete and disable actions? [Completeness, Spec §FR-018]
- [ ] CHK008 Is the "无权限提示页面" specified with required content (message, return-home action, role context)? [Gap, Spec §US3 AS-4]
- [ ] CHK009 Are session-expiry UX requirements specified — does the user see a toast/modal before redirect, or silent redirect; is the in-progress form preserved? [Gap, Spec §FR-019]
- [ ] CHK010 Are password-input UX requirements specified (masking, show/hide toggle, paste behavior, strength meter)? [Gap, Spec §FR-001, §FR-006]

## Requirement Clarity — UX

- [ ] CHK011 Is "实时或刷新后显示最新数据" quantified — real-time push, polling interval, or manual refresh only? [Ambiguity, Spec §US4 AS-3]
- [ ] CHK012 Is the timezone for `YYYY-MM-DD HH:mm:ss` timestamps explicitly stated (server UTC, browser local, fixed CST)? [Ambiguity, Spec §Edge Cases]
- [ ] CHK013 Is "在线管理员" defined with a measurable rule (e.g. last_login within N minutes, active JWT) so the dashboard number is reproducible? [Clarity, Spec §FR-014]
- [ ] CHK014 Is "今日登录次数" defined with timezone and de-duplication rules (counts per attempt, per success, or per unique admin)? [Clarity, Spec §FR-014]
- [ ] CHK015 Are the "添加、编辑、删除、禁用/启用按钮" UX states for each role described as hidden vs disabled vs error-on-click? [Clarity, Spec §US3 AS-1/2/3]

## Requirement Consistency — UX

- [ ] CHK016 Do the role-visibility rules agree between §US3 AS-1 ("看不到") and §US3 AS-2 ("禁用状态或不可见")? Pick one convention or document when each applies. [Conflict, Spec §US3]
- [ ] CHK017 Are the columns shown in the admin list (§FR-010: ID/phone/nickname/status/created/last-login) consistent with the fields editable in §FR-007 (phone/nickname/status) — is "角色" intentionally omitted from list view and edit form? [Consistency, Spec §FR-007, §FR-010]
- [ ] CHK018 Is the destructive-action wording consistent (删除 vs 移除, 禁用 vs 停用) across confirmation prompts, button labels, and error messages? [Consistency, Spec §US2, §FR-008, §FR-009]

## Acceptance Criteria Quality — UX

- [ ] CHK019 Is "10 秒内完成登录流程" measurable end-to-end — what starts and stops the timer (first paint, form-ready, post-redirect dashboard render)? [Measurability, Spec §SC-001]
- [ ] CHK020 Can "95% 的管理员无需培训即可完成基本操作" be objectively measured, or does it need a usability test protocol? [Measurability, Spec §SC-004]
- [ ] CHK021 Are §SC-002 (admin list ≤ 2s) and §SC-003 (dashboard ≤ 3s) specified relative to network conditions, dataset size, and device class? [Clarity, Spec §SC-002/3]

## Scenario Coverage — UX states

- [ ] CHK022 Are primary success-flow UX requirements complete for each user story (US1 login → dashboard, US2 CRUD, US3 menu render, US4 stats)? [Coverage]
- [ ] CHK023 Are alternate-flow UX requirements specified (e.g. login with auto-fill, re-submit after correcting a single field, editing self vs editing other)? [Coverage, Gap]
- [ ] CHK024 Are exception-flow UX requirements specified for partial failure (one stats card fails to load while others succeed)? [Gap, Spec §US4]
- [ ] CHK025 Are recovery-flow UX requirements specified (after a 401 mid-CRUD, after a network drop during dashboard refresh)? [Gap]

## Edge Case Coverage — UX

- [ ] CHK026 Are display requirements specified for `last_login_at = NULL` (a brand-new admin who has never logged in)? [Edge Case, Gap, Spec §FR-010]
- [ ] CHK027 Are display requirements specified for long nicknames (up to 20 chars) and how columns truncate / wrap / tooltip? [Edge Case, Gap]
- [ ] CHK028 Are the UX requirements for "至少保留一个启用的超级管理员" defined — is the disable/delete button hidden, disabled with tooltip, or shown with a post-click error? [Edge Case, Spec §FR-012]
- [ ] CHK029 Are UX requirements defined for the locked-out state (FR-017) — countdown timer shown to the user, generic message, or admin contact prompt? [Gap, Spec §FR-017]

## Non-Functional UX — Accessibility, Responsive, i18n, Theming

- [ ] CHK030 Are accessibility requirements specified (keyboard navigation, focus order, ARIA labels, color-contrast targets, screen-reader behavior for tables and dialogs)? [Gap]
- [ ] CHK031 Are responsive-layout requirements specified — supported breakpoints, mobile vs tablet vs desktop behavior, minimum supported width? [Gap]
- [ ] CHK032 Are internationalization requirements specified — is Simplified Chinese the only supported locale, or must copy be externalized for future locales? [Gap]
- [ ] CHK033 Are theming requirements specified (light only, light + dark, brand colors, customization scope)? [Gap]
- [ ] CHK034 Are browser-support requirements specified (which browsers and versions must render the UI)? [Gap]

## Implementation-vs-Spec Gaps (informed by current code)

- [ ] CHK035 The implementation uses Ant Design as the UI library (per `plan.md` research). Is the choice of component library — or at least the visual-language standards it implies (spacing, button hierarchy, form layout) — referenced anywhere in spec.md? [Gap]
- [ ] CHK036 The admin-list endpoint defaults to `page_size=10` (per `quickstart.md` and code). Is this default codified in spec.md, or is it an implementation choice the spec leaves open? [Gap, Spec §FR-010]
- [ ] CHK037 The frontend stores the JWT in browser storage (per `web/src/utils/auth.ts`). Are the UX implications (auto-login on return visit, log-out-everywhere, multi-tab behavior) specified? [Gap, Spec §FR-019]
- [ ] CHK038 The implementation exposes a `PermissionGuard` component and redirects on insufficient permission. Does spec.md describe the UX of that redirect (target page, message, breadcrumb) or only that it occurs? [Gap, Spec §US3 AS-4]
- [ ] CHK039 The dashboard component renders stats cards and a recent-logins table (per `web/src/components/Dashboard.tsx`). Are the visual hierarchy, ordering, and grouping of these UI elements specified in spec.md, or chosen at implementation time? [Gap, Spec §FR-013/14/15]
- [ ] CHK040 The `useAuth` hook performs role-based redirects on login. Is the post-login landing page per role specified (does Normal land on dashboard, same as Super)? [Gap, Spec §US1 AS-1]

## Ambiguities & Traceability

- [ ] CHK041 Is there a stable ID scheme that lets each UX-affecting requirement (e.g. "destructive button styling", "empty-state copy") be referenced from designs, code, and tests? [Traceability]
- [ ] CHK042 Are open UX questions (the items flagged `[Gap]` above) tracked somewhere — a clarifications log, follow-up issues, or an explicit "deferred to v2" list — so reviewers know they were considered? [Traceability]

---

## Notes

- Check items off as you resolve them in `spec.md` (or document a deliberate "out of scope" decision next to the item).
- This checklist tests the **requirements**, not the implementation. An item is "passing" when the spec answers it — not when the code does.
- Gaps surfaced from the implementation should be back-filled into `spec.md` so future contributors don't have to read code to learn the intent.
