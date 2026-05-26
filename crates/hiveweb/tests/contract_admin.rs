//! Contract tests for /api/admins/* (T026c, T026d, T026e).
//!
//! Phase 2.5 RED.

mod common;

use serde_json::json;

#[tokio::test]
async fn t026c_admins_list_requires_auth() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let (status, _) = common::get(&app, "/api/admins?page=1&page_size=10", None).await?;
    assert_eq!(status, 401, "unauthenticated /admins must return 401");
    Ok(())
}

#[tokio::test]
async fn t026c_admins_list_normal_role_can_view() -> anyhow::Result<()> {
    // Until a helper that mints a JWT for a Normal admin is wired up,
    // this test fails by design — it is a placeholder for the role-aware
    // listing contract (Normal sees the list, no write controls).
    panic!(
        "Phase 2.5 RED: requires test fixture for Normal-role JWT \
         (see specs/003-admin-center/spec.md §US3 AS-1)"
    );
}

#[tokio::test]
async fn t026d_create_admin_rejects_invalid_phone() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let (status, body) = common::post_json(
        &app,
        "/api/admins",
        json!({ "phone": "not-a-phone", "nickname": "x", "password": "secret", "role": 1 }),
    )
    .await?;
    assert!(
        status.is_client_error(),
        "invalid phone must be rejected, got {status}: {body}"
    );
    Ok(())
}

#[tokio::test]
async fn t026d_create_admin_rejects_duplicate_phone() -> anyhow::Result<()> {
    panic!(
        "Phase 2.5 RED: requires DB seed with an existing admin to assert error code 3002 \
         (see spec.md FR-011)"
    );
}

#[tokio::test]
async fn t026e_delete_super_admin_is_forbidden() -> anyhow::Result<()> {
    panic!(
        "Phase 2.5 RED: requires DB seed with Super admin to assert error code 3003 \
         (see spec.md FR-009, FR-012)"
    );
}

#[tokio::test]
async fn t026e_patch_status_validates_payload() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let (status, _) = common::post_json(
        &app,
        "/api/admins/9999/status",
        json!({ "status": 42 }),
    )
    .await?;
    assert!(
        status.is_client_error(),
        "PATCH status must reject out-of-range status value, got {status}"
    );
    Ok(())
}
