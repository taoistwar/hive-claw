//! Integration test: account lockout after 5 failures (T026g, FR-017).
//!
//! Phase 2.5 RED.

mod common;

use serde_json::json;

#[tokio::test]
async fn t026g_lockout_engages_after_five_failures_and_expires_after_fifteen_minutes() -> anyhow::Result<()> {
    let app = common::test_app().await?;

    // Pump 5 consecutive failures.
    for attempt in 1..=5 {
        let (status, _) = common::post_json(
            &app,
            "/api/auth/login",
            json!({ "phone": "13800138000", "password": format!("bad-{attempt}") }),
        )
        .await?;
        assert!(
            status.is_client_error(),
            "attempt {attempt} should fail with 4xx, got {status}"
        );
    }

    // 6th attempt — must be 1003 (account locked) even with the correct password.
    let (status, body) = common::post_json(
        &app,
        "/api/auth/login",
        json!({ "phone": "13800138000", "password": "admin123" }),
    )
    .await?;
    assert!(status.is_client_error(), "6th attempt must be rejected, got {status}");
    assert_eq!(body["code"], 1003, "after 5 failures, code must be 1003 (locked)");

    // 15-minute expiry verification deferred to a time-controllable harness.
    panic!(
        "Phase 2.5 RED: 15-minute expiry leg of FR-017 needs a clock-injection \
         test harness (see services/auth.rs); marking RED until that lands."
    );
}
