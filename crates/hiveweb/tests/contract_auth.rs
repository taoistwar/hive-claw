//! Contract tests for /api/auth/* (T026a, T026b).

mod common;

use serde_json::json;

const TEST_PASSWORD: &str = "test-pass-123";

#[tokio::test]
async fn t026a_login_returns_token_on_valid_credentials() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 1, 1, TEST_PASSWORD).await?;

    let (status, body) = common::post_json(
        &app,
        "/api/auth/login",
        json!({ "phone": admin.phone, "password": TEST_PASSWORD }),
    )
    .await?;

    assert_eq!(
        status, 200,
        "expected 200 OK for valid login, got {status}: {body}"
    );
    assert_eq!(body["code"], 0, "expected code=0 on success");
    assert!(
        body["data"]["token"].is_string(),
        "expected token string in response"
    );
    assert!(
        body["data"]["admin"].is_object(),
        "expected admin object in response"
    );
    Ok(())
}

#[tokio::test]
async fn t026a_login_rejects_wrong_password() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 1, 1, TEST_PASSWORD).await?;

    let (status, body) = common::post_json(
        &app,
        "/api/auth/login",
        json!({ "phone": admin.phone, "password": "not-the-real-password" }),
    )
    .await?;

    assert!(
        status.is_client_error(),
        "expected 4xx for wrong password, got {status}: {body}"
    );
    assert_eq!(
        body["code"], 1001,
        "expected error code 1001 (wrong password)"
    );
    Ok(())
}

#[tokio::test]
async fn t026a_login_locks_after_five_failures() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 1, 1, TEST_PASSWORD).await?;

    for _ in 0..5 {
        let _ = common::post_json(
            &app,
            "/api/auth/login",
            json!({ "phone": admin.phone, "password": "wrong-pw" }),
        )
        .await?;
    }

    let (status, body) = common::post_json(
        &app,
        "/api/auth/login",
        json!({ "phone": admin.phone, "password": "wrong-pw" }),
    )
    .await?;

    assert!(
        status.is_client_error(),
        "expected 4xx after lockout, got {status}: {body}"
    );
    assert_eq!(
        body["code"], 1003,
        "expected error code 1003 (account locked)"
    );
    Ok(())
}

#[tokio::test]
async fn t026b_me_requires_token() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let (status, _) = common::get(&app, "/api/auth/me", None).await?;
    assert_eq!(status, 401, "unauthenticated /me must return 401");
    Ok(())
}

#[tokio::test]
async fn t026b_me_rejects_invalid_token() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let (status, body) = common::get(&app, "/api/auth/me", Some("not-a-real-jwt")).await?;
    assert_eq!(
        status, 401,
        "invalid token must return 401, got {status}: {body}"
    );
    Ok(())
}
