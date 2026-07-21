use std::path::{Path, PathBuf};

use axum::{Router, body::Body, http::Request};
use hiveweb::{app_mode::AppMode, web_admin};
use http_body_util::BodyExt;
use tower::ServiceExt;

#[test]
fn development_uses_workspace_web_admin_dist() {
    let executable = Path::new("/tmp/target/debug/hiveweb");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");

    assert_eq!(
        web_admin::dist_dir_for(AppMode::Development, executable),
        workspace.join("web-admin/dist")
    );
}

#[test]
fn test_and_production_use_dist_next_to_executable() {
    let executable = Path::new("/opt/hive-claw/bin/hiveweb");
    let expected = PathBuf::from("/opt/hive-claw/bin/dist");

    assert_eq!(web_admin::dist_dir_for(AppMode::Test, executable), expected);
    assert_eq!(
        web_admin::dist_dir_for(AppMode::Production, executable),
        expected
    );
}

#[tokio::test]
async fn serves_assets_and_falls_back_to_spa_index() {
    let dist = std::env::temp_dir().join(format!("hiveweb-static-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dist.join("assets")).expect("create dist");
    std::fs::write(dist.join("index.html"), "<html>admin</html>").expect("write index");
    std::fs::write(dist.join("assets/app.js"), "console.log('admin')").expect("write asset");

    let app = web_admin::serve_dist(Router::new(), &dist);

    let root = app
        .clone()
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(root.status(), 404);

    let entry = app
        .clone()
        .oneshot(Request::get("/web-admin").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(entry.status(), 200);
    assert_eq!(
        entry.into_body().collect().await.unwrap().to_bytes(),
        "<html>admin</html>"
    );

    let asset = app
        .clone()
        .oneshot(
            Request::get("/web-admin/assets/app.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(asset.status(), 200);
    assert_eq!(
        asset.into_body().collect().await.unwrap().to_bytes(),
        "console.log('admin')"
    );

    let spa = app
        .oneshot(
            Request::get("/web-admin/admins/1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(spa.status(), 200);
    assert_eq!(
        spa.into_body().collect().await.unwrap().to_bytes(),
        "<html>admin</html>"
    );

    std::fs::remove_dir_all(dist).expect("remove dist");
}

#[tokio::test]
async fn spa_fallback_does_not_handle_backend_routes() {
    let dist = std::env::temp_dir().join(format!("hiveweb-static-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dist).expect("create dist");
    std::fs::write(dist.join("index.html"), "<html>admin</html>").expect("write index");

    let database = sqlx::MySqlPool::connect_lazy("mysql://static:static@127.0.0.1:1/static")
        .expect("lazy MySQL pool");
    let redis = redis::Client::open("redis://127.0.0.1:1").expect("lazy Redis client");
    let app = hiveweb::api::create_router(
        database,
        redis.into(),
        None,
        None,
        hiveweb::services::sensitive_filter::SensitiveFilter::new(),
    );
    let app = web_admin::serve_dist(app, &dist);

    for path in ["/api/does-not-exist", "/health/does-not-exist"] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 404, "{path}");
    }

    std::fs::remove_dir_all(dist).expect("remove dist");
}
