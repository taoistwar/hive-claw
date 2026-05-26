//! Integration test: account lockout after 5 failures (T026g, FR-017).

mod common;

use serde_json::json;

#[tokio::test]
async fn t026g_lockout_engages_after_five_failures() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 1, 1, "test-pass-123").await?;

    for attempt in 1..=5 {
        let (status, _) = common::post_json(
            &app,
            "/api/auth/login",
            json!({ "phone": admin.phone, "password": format!("bad-{attempt}") }),
        )
        .await?;
        assert!(
            status.is_client_error(),
            "attempt {attempt} should fail with 4xx, got {status}"
        );
    }

    // 6th attempt — must return 1003 (account locked) even with the correct password.
    let (status, body) = common::post_json(
        &app,
        "/api/auth/login",
        json!({ "phone": admin.phone, "password": "test-pass-123" }),
    )
    .await?;
    assert!(status.is_client_error(), "6th attempt must be rejected, got {status}");
    assert_eq!(body["code"], 1003, "after 5 failures, code must be 1003 (locked)");

    // The 15-minute expiry leg of FR-017 needs a clock-injection harness to
    // verify deterministically; deferred. We've at least pinned the lockout
    // trigger behavior here.
    Ok(())
}
