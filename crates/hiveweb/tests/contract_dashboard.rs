//! Contract tests for /api/dashboard/* (T026f).

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
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let super_admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;

    let (status, body) = common::get(
        &app,
        "/api/dashboard/stats",
        Some(&super_admin.token()?),
    )
    .await?;

    assert_eq!(status, 200, "/dashboard/stats must return 200 for Super, got {status}: {body}");
    let data = &body["data"];
    assert!(data.is_object(), "expected object payload, got {body}");
    // Spec FR-014 (post-2026-05-26): total_admins, online_admins, today_logins.
    for key in ["total_admins", "online_admins", "today_logins"] {
        assert!(
            data[key].is_number(),
            "spec FR-014 requires integer field `{key}` in stats payload; got {body}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn t026f_dashboard_recent_logins_returns_array() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let super_admin = common::seed_admin(&pool, 3, 1, "test-pass-123").await?;

    let (status, body) = common::get(
        &app,
        "/api/dashboard/recent-logins",
        Some(&super_admin.token()?),
    )
    .await?;

    assert_eq!(status, 200, "/dashboard/recent-logins must return 200, got {status}: {body}");
    assert!(
        body["data"].is_array(),
        "spec FR-015 requires array payload for recent-logins; got {body}"
    );
    let len = body["data"].as_array().unwrap().len();
    assert!(
        len <= 10,
        "spec FR-015 caps recent logins at 10, got {len}: {body}"
    );
    Ok(())
}
