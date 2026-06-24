//! Contract tests for POST /api/auth/change-password (T103).

mod common;

use serde_json::json;

const TEST_PASSWORD: &str = "test-pass-123";
const NEW_PASSWORD: &str = "new-pass-456";

#[tokio::test]
async fn t103_change_password_success() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 1, 1, TEST_PASSWORD).await?;

    let token = admin.token()?;

    let (status, body) = common::post_json_auth(
        &app,
        "/api/auth/change-password",
        &token,
        json!({ "old_password": TEST_PASSWORD, "new_password": NEW_PASSWORD }),
    )
    .await?;

    assert_eq!(
        status, 200,
        "expected 200 OK for valid password change, got {status}: {body}"
    );
    assert_eq!(body["code"], 0, "expected code=0 on success");
    Ok(())
}

#[tokio::test]
async fn t103_change_password_wrong_old_password() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 1, 1, TEST_PASSWORD).await?;

    let token = admin.token()?;

    let (status, body) = common::post_json_auth(
        &app,
        "/api/auth/change-password",
        &token,
        json!({ "old_password": "wrong-old-pw", "new_password": NEW_PASSWORD }),
    )
    .await?;

    assert!(
        status.is_client_error(),
        "expected 4xx for wrong old password, got {status}: {body}"
    );
    assert_eq!(
        body["code"], 1001,
        "expected error code 1001 (wrong password)"
    );
    Ok(())
}

#[tokio::test]
async fn t103_change_password_same_as_old() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 1, 1, TEST_PASSWORD).await?;

    let token = admin.token()?;

    let (status, body) = common::post_json_auth(
        &app,
        "/api/auth/change-password",
        &token,
        json!({ "old_password": TEST_PASSWORD, "new_password": TEST_PASSWORD }),
    )
    .await?;

    assert!(
        status.is_client_error(),
        "expected 4xx when new password same as old, got {status}: {body}"
    );
    assert_eq!(
        body["code"], 3008,
        "expected error code 3008 (new password same as old)"
    );
    Ok(())
}

#[tokio::test]
async fn t103_change_password_requires_auth() -> anyhow::Result<()> {
    let app = common::test_app().await?;

    let (status, _) = common::post_json(
        &app,
        "/api/auth/change-password",
        json!({ "old_password": TEST_PASSWORD, "new_password": NEW_PASSWORD }),
    )
    .await?;

    assert_eq!(
        status, 401,
        "unauthenticated change-password must return 401"
    );
    Ok(())
}
