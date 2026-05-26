//! Integration test: role-based access control isolation (T026h, SC-007).
//!
//! Phase 2.5 RED — requires JWT fixtures for each role.

mod common;

#[tokio::test]
async fn t026h_normal_role_cannot_create_admin() -> anyhow::Result<()> {
    panic!(
        "Phase 2.5 RED: requires Normal-role JWT fixture; \
         then POST /api/admins must return 403 + code 2001 (spec.md FR-005, §US3 AS-4)"
    );
}

#[tokio::test]
async fn t026h_system_role_cannot_delete_admin() -> anyhow::Result<()> {
    panic!(
        "Phase 2.5 RED: requires System-role JWT fixture; \
         then DELETE /api/admins/:id must return 403 + code 2001 (spec.md FR-009, §US3 AS-2)"
    );
}

#[tokio::test]
async fn t026h_super_role_can_perform_all_operations() -> anyhow::Result<()> {
    panic!(
        "Phase 2.5 RED: requires Super-role JWT fixture; \
         then list/create/update/delete/toggle endpoints all return 2xx"
    );
}

#[tokio::test]
async fn t026h_cross_role_smoke_no_privilege_escalation() -> anyhow::Result<()> {
    panic!(
        "Phase 2.5 RED: SC-007 — verify 100% of cross-role probes return 403; \
         requires the JWT fixtures above"
    );
}
