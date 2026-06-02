//! Integration test: login_records + last_login_at side effects (T026j, FR-003, FR-015).

mod common;

use serde_json::json;

#[tokio::test]
async fn t026j_successful_login_updates_last_login_at_and_writes_record() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 1, 1, "test-pass-123").await?;

    let (status, body) = common::post_json(
        &app,
        "/api/auth/login",
        json!({ "phone": admin.phone, "password": "test-pass-123" }),
    )
    .await?;
    assert_eq!(
        status, 200,
        "login must succeed for seeded admin, got {status}: {body}"
    );

    // (a) admins.last_login_at advanced
    let row: (Option<chrono::NaiveDateTime>,) =
        sqlx::query_as("SELECT last_login_at FROM admins WHERE id = ?")
            .bind(admin.id)
            .fetch_one(&pool)
            .await?;
    assert!(
        row.0.is_some(),
        "FR-003: last_login_at must be set after a successful login"
    );

    // (b) a login_records row written with success=1
    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM login_records WHERE admin_id = ? AND success = 1")
            .bind(admin.id)
            .fetch_one(&pool)
            .await?;
    assert!(
        count.0 >= 1,
        "FR-015: successful login must produce a login_records row, got {} rows",
        count.0
    );

    Ok(())
}

#[tokio::test]
async fn t026j_failed_login_writes_record_with_failure_reason() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 1, 1, "test-pass-123").await?;

    let _ = common::post_json(
        &app,
        "/api/auth/login",
        json!({ "phone": admin.phone, "password": "WRONG-passwd" }),
    )
    .await?;

    // Existing behaviour: services::auth::create_login_record is called with
    // success=false and Some("wrong_password"). Assert the row landed.
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM login_records \
         WHERE admin_id = ? AND success = 0 AND failure_reason IS NOT NULL",
    )
    .bind(admin.id)
    .fetch_one(&pool)
    .await?;
    assert!(
        count.0 >= 1,
        "failed login must persist a login_records row with failure_reason; got {} rows",
        count.0
    );
    Ok(())
}
