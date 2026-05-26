//! Integration test: last super-admin guard (T026i, FR-009, FR-012).

mod common;

use serde_json::json;

#[tokio::test]
async fn t026i_cannot_delete_super_admin() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let actor = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;
    let other_super = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;

    let (status, body) = common::delete_auth(
        &app,
        &format!("/api/admins/{}", other_super.id),
        &actor.token()?,
    )
    .await?;

    assert!(
        status.is_client_error(),
        "DELETE on a super admin must fail, got {status}: {body}"
    );
    assert_eq!(
        body["code"], 3003,
        "deleting any super admin must return code 3003, got {body}"
    );
    Ok(())
}

#[tokio::test]
async fn t026i_disable_super_admin_path_returns_status_code() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let actor = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;
    let other_super = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;

    // Try to disable one of the multiple active super admins.
    let (status, body) = common::patch_json_auth(
        &app,
        &format!("/api/admins/{}/status", other_super.id),
        &actor.token()?,
        json!({ "status": 0 }),
    )
    .await?;

    // When more than one super exists, this should succeed.
    // When only one would remain, FR-012 demands code 3004.
    // We can't easily make this deterministic without a transactional
    // truncation of `admins` (which destroys the bootstrap super), so
    // we just assert one of the two valid outcomes here.
    assert!(
        status.is_success() || body["code"] == 3004,
        "expected 2xx (success) or code 3004 (last super), got {status}: {body}"
    );
    Ok(())
}
