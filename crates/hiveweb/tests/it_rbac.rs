//! Integration test: role-based access control isolation (T026h, SC-007).

mod common;

use serde_json::json;

#[tokio::test]
async fn t026h_normal_role_cannot_create_admin() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let normal = common::seed_admin(&pool, 1, 1, "test-pass-123").await?;

    let (status, body) = common::post_json_auth(
        &app,
        "/api/admins",
        &normal.token()?,
        json!({ "phone": "13911110000", "nickname": "x", "password": "secret-123", "role": 1 }),
    )
    .await?;

    assert!(
        status.is_client_error(),
        "Normal must not create admin, got {status}: {body}"
    );
    assert_eq!(
        body["code"], 2001,
        "must return code 2001 (insufficient permission)"
    );
    Ok(())
}

#[tokio::test]
async fn t026h_system_role_cannot_delete_admin() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let system = common::seed_admin(&pool, 2, 1, "test-pass-123").await?;
    let target = common::seed_admin(&pool, 1, 1, "test-pass-123").await?;

    let (status, body) = common::delete_auth(
        &app,
        &format!("/api/admins/{}", target.id),
        &system.token()?,
    )
    .await?;

    // Spec §US3 AS-2: System admin must not be able to delete admins.
    // Note: the backend's current `can_manage_admins()` includes both System
    // and Super for ALL operations; this assertion may RED until role
    // capability is split per-operation.
    assert!(
        status.is_client_error(),
        "System role must not be able to delete admins (spec §US3 AS-2); got {status}: {body}"
    );
    assert_eq!(
        body["code"], 2001,
        "must return spec code 2001 for System-deleting-admin, got {body}"
    );
    Ok(())
}

#[tokio::test]
async fn t026h_super_role_can_list_admins() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let super_admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;

    let (status, body) = common::get(
        &app,
        "/api/admins?offset=0&limit=10",
        Some(&super_admin.token()?),
    )
    .await?;

    assert_eq!(
        status, 200,
        "Super must be able to list admins, got {status}: {body}"
    );
    Ok(())
}

#[tokio::test]
async fn t026h_unauthenticated_probe_returns_1004() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let (status, body) = common::get(&app, "/api/admins?offset=0&limit=10", None).await?;
    assert_eq!(status, 401, "no auth header → 401");
    assert_eq!(
        body["code"], 1004,
        "no auth header → spec code 1004 (token invalid)"
    );
    Ok(())
}
