//! Contract tests for /api/dashboard/* (T026f).
//!
//! Phase 2.5 RED.

mod common;

#[tokio::test]
async fn t026f_dashboard_stats_requires_auth() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let (status, _) = common::get(&app, "/api/dashboard/stats", None).await?;
    assert_eq!(status, 401, "unauthenticated /dashboard/stats must return 401");
    Ok(())
}

#[tokio::test]
async fn t026f_dashboard_stats_shape_matches_spec() -> anyhow::Result<()> {
    // Spec FR-014 mandates: total_admins, online_admins, today_logins (with
    // the precise definitions added in the 2026-05-26 spec revision).
    panic!(
        "Phase 2.5 RED: requires authenticated Super JWT fixture; \
         then assert body.data has integer fields total_admins, online_admins, today_logins"
    );
}

#[tokio::test]
async fn t026f_dashboard_recent_logins_returns_up_to_ten() -> anyhow::Result<()> {
    // Spec FR-015 (post-2026-05-26) requires the most-recent 10 entries with
    // the new admin_phone_snapshot / admin_nickname_snapshot fields (T096).
    panic!(
        "Phase 2.5 RED: requires authenticated JWT fixture AND T096 migration \
         (login_records snapshot columns)"
    );
}
