//! Integration test for password change flow (T104).

mod common;

use serde_json::json;

const TEST_PASSWORD: &str = "test-pass-123";
const NEW_PASSWORD: &str = "new-pass-456";

#[tokio::test]
async fn t104_login_with_new_password_after_change() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 1, 1, TEST_PASSWORD).await?;

    let token = admin.token()?;

    let (status, _) = common::post_json_auth(
        &app,
        "/api/auth/change-password",
        &token,
        json!({ "old_password": TEST_PASSWORD, "new_password": NEW_PASSWORD }),
    )
    .await?;

    assert_eq!(status, 200, "password change must succeed");

    let (status, body) = common::post_json(
        &app,
        "/api/auth/login",
        json!({ "phone": admin.phone, "password": NEW_PASSWORD }),
    )
    .await?;

    assert_eq!(
        status, 200,
        "login with new password must succeed, got {status}: {body}"
    );
    assert_eq!(body["code"], 0, "expected code=0 on login success");
    Ok(())
}

#[tokio::test]
async fn t104_login_with_old_password_fails_after_change() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 1, 1, TEST_PASSWORD).await?;

    let token = admin.token()?;

    let (status, _) = common::post_json_auth(
        &app,
        "/api/auth/change-password",
        &token,
        json!({ "old_password": TEST_PASSWORD, "new_password": NEW_PASSWORD }),
    )
    .await?;

    assert_eq!(status, 200, "password change must succeed");

    let (status, body) = common::post_json(
        &app,
        "/api/auth/login",
        json!({ "phone": admin.phone, "password": TEST_PASSWORD }),
    )
    .await?;

    assert!(
        status.is_client_error(),
        "login with old password must fail after change, got {status}: {body}"
    );
    assert_eq!(
        body["code"], 1001,
        "expected error code 1001 (wrong password)"
    );
    Ok(())
}
