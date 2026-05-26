//! Integration test: login_records + last_login_at side effects (T026j, FR-003, FR-015).
//!
//! Phase 2.5 RED — also asserts T096 columns (admin_phone_snapshot,
//! admin_nickname_snapshot) which currently do NOT exist; that's intentional.

mod common;

use serde_json::json;

#[tokio::test]
async fn t026j_successful_login_updates_last_login_at_and_writes_record() -> anyhow::Result<()> {
    let app = common::test_app().await?;

    let (status, _) = common::post_json(
        &app,
        "/api/auth/login",
        json!({ "phone": "13800138000", "password": "admin123" }),
    )
    .await?;
    assert_eq!(status, 200, "login must succeed for the seeded super admin");

    panic!(
        "Phase 2.5 RED: requires direct DB read to assert \
         (a) admins.last_login_at advanced, \
         (b) a new login_records row with success=1, \
         (c) post-T096: admin_phone_snapshot + admin_nickname_snapshot populated."
    );
}

#[tokio::test]
async fn t026j_failed_login_writes_record_with_failure_reason() -> anyhow::Result<()> {
    let app = common::test_app().await?;

    let _ = common::post_json(
        &app,
        "/api/auth/login",
        json!({ "phone": "13800138000", "password": "WRONG" }),
    )
    .await?;

    panic!(
        "Phase 2.5 RED: requires direct DB read to assert a login_records row \
         with success=0 and failure_reason='WRONG_PASSWORD' (spec.md §LoginRecord, ambiguity U4)"
    );
}
