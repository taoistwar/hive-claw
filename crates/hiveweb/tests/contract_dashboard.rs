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
    // Wire format: camelCase (DashboardStats serde renames; see
    // services/dashboard.rs). `activeAdmins` is the spec FR-014
    // "online_admins" (status=1 AND last_login_at within 24h) field;
    // `disabledAdmins` is an additional convenience field the frontend uses.
    for key in ["totalAdmins", "activeAdmins", "todayLogins", "disabledAdmins"] {
        assert!(
            data[key].is_number(),
            "stats payload must include integer field `{key}`; got {body}"
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
