//! Integration test: last super-admin guard (T026i, FR-009, FR-012).
//!
//! Phase 2.5 RED.

mod common;

#[tokio::test]
async fn t026i_cannot_delete_super_admin() -> anyhow::Result<()> {
    panic!(
        "Phase 2.5 RED: requires Super-role JWT + DB with a super admin; \
         DELETE /api/admins/:super_id must return error code 3003 (spec.md FR-009)"
    );
}

#[tokio::test]
async fn t026i_cannot_disable_last_active_super_admin() -> anyhow::Result<()> {
    panic!(
        "Phase 2.5 RED: requires Super-role JWT + DB with exactly one active super admin; \
         PATCH /api/admins/:super_id/status to disabled must return error code 3004 (spec.md FR-012)"
    );
}

#[tokio::test]
async fn t026i_can_disable_super_admin_when_another_active_super_exists() -> anyhow::Result<()> {
    panic!(
        "Phase 2.5 RED: requires DB with two active super admins; \
         disabling one must succeed (spec.md FR-012 boundary)"
    );
}
