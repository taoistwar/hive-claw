//! Integration test for password change lockout after 5 wrong old-password attempts (T105).

mod common;

use serde_json::json;

const TEST_PASSWORD: &str = "test-pass-123";
const NEW_PASSWORD: &str = "new-pass-456";

#[tokio::test]
async fn t105_lockout_after_five_wrong_old_password_attempts() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 1, 1, TEST_PASSWORD).await?;

    let token = admin.token()?;

    for _ in 0..5 {
        let _ = common::post_json_auth(
            &app,
            "/api/auth/change-password",
            &token,
            json!({ "old_password": "wrong-pw", "new_password": NEW_PASSWORD }),
        )
        .await?;
    }

    let (status, body) = common::post_json_auth(
        &app,
        "/api/auth/change-password",
        &token,
        json!({ "old_password": TEST_PASSWORD, "new_password": NEW_PASSWORD }),
    )
    .await?;

    assert!(
        status.is_client_error(),
        "expected 4xx after lockout, got {status}: {body}"
    );
    assert_eq!(body["code"], 1003, "expected error code 1003 (account locked)");
    Ok(())
}
