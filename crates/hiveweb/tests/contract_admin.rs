//! Contract tests for /api/admins/* (T026c, T026d, T026e).

mod common;

use serde_json::json;

#[tokio::test]
async fn t026c_admins_list_requires_auth() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let (status, _) = common::get(&app, "/api/admins?offset=0&limit=10", None).await?;
    assert_eq!(status, 401, "unauthenticated /admins must return 401");
    Ok(())
}

#[tokio::test]
async fn t026c_admins_list_normal_role_can_view() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let normal = common::seed_admin(&pool, 1 /* Normal */, 1, "test-pass-123").await?;

    let (status, body) = common::get(
        &app,
        "/api/admins?offset=0&limit=10",
        Some(&normal.token()?),
    )
    .await?;

    // Per spec §US3 AS-1, a Normal admin should still be able to *view* the list
    // (the action buttons are hidden client-side). Backend permission rule:
    // `can_manage_admins()`. If the current implementation forbids GET for
    // Normal, this assertion correctly fails RED and points to the gap.
    assert_eq!(
        status, 200,
        "Normal role should be able to GET the admin list; got {status}: {body}"
    );
    Ok(())
}

#[tokio::test]
async fn t026d_create_admin_rejects_invalid_phone() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let super_admin = common::seed_admin(&pool, 3 /* Super */, 1, "test-pass-123").await?;

    let (status, body) = common::post_json_auth(
        &app,
        "/api/admins",
        &super_admin.token()?,
        json!({ "phone": "not-a-phone", "nickname": "x", "password": "secret123", "role": 1 }),
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
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let super_admin = common::seed_admin(&pool, 3 /* Super */, 1, "test-pass-123").await?;
    let existing = common::seed_admin(&pool, 1 /* Normal */, 1, "test-pass-123").await?;

    let (status, body) = common::post_json_auth(
        &app,
        "/api/admins",
        &super_admin.token()?,
        json!({
            "phone": existing.phone,
            "nickname": "dup",
            "password": "test-pass-123",
            "role": 1,
        }),
    )
    .await?;

    assert!(
        status.is_client_error(),
        "duplicate phone must be rejected, got {status}: {body}"
    );
    assert_eq!(
        body["code"], 3002,
        "duplicate phone must return spec error code 3002, got {body}"
    );
    Ok(())
}

#[tokio::test]
async fn t026e_delete_super_admin_is_forbidden() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let actor = common::seed_admin(&pool, 3 /* Super */, 1, "test-pass-123").await?;
    let victim = common::seed_admin(&pool, 3 /* Super */, 1, "test-pass-123").await?;

    let (status, body) =
        common::delete_auth(&app, &format!("/api/admins/{}", victim.id), &actor.token()?).await?;

    assert!(
        status.is_client_error(),
        "deleting a super admin must be rejected, got {status}: {body}"
    );
    assert_eq!(
        body["code"], 3003,
        "must return spec error code 3003 (cannot delete super admin), got {body}"
    );
    Ok(())
}

#[tokio::test]
async fn t026e_patch_status_validates_payload() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let super_admin = common::seed_admin(&pool, 3 /* Super */, 1, "test-pass-123").await?;

    let (status, _) = common::patch_json_auth(
        &app,
        "/api/admins/9999/status",
        &super_admin.token()?,
        json!({ "status": 42 }),
    )
    .await?;
    assert!(
        status.is_client_error(),
        "PATCH status must reject out-of-range status value, got {status}"
    );
    Ok(())
}
