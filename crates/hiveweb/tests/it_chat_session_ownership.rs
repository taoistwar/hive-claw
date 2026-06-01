//! Chat session ownership & isolation integration test (T164 / FR-027 v7)
//!
//! Verifies:
//!   1. admin-A creates session → admin-B POST messages → 403
//!   2. admin-A can read/write own session normally
//!   3. Super can read other admin's session messages
//!   4. Non-Super reading other admin's session → 403

mod common;

use common::{mint_jwt, seed_admin, post_json_auth, get, post_json, delete_auth};
use axum::Router;
use axum::http::StatusCode;
use serde_json::{json, Value};

/// Helper: create a chat session as a given admin, return session id.
async fn create_session(app: &Router, token: &str) -> anyhow::Result<i64> {
    let (status, body) = post_json_auth(
        app,
        "/api/admin-chat/sessions",
        token,
        json!({"title": "test-session"}),
    )
    .await?;
    assert_eq!(status, 200, "create session failed: {status} {body}");
    Ok(body["data"]["id"].as_i64().expect("missing session id"))
}

/// Helper: POST a message to a chat session (just to trigger ownership check;
/// we don't need the SSE stream to complete for this test).
async fn post_message(app: &Router, token: &str, session_id: i64) -> anyhow::Result<(StatusCode, Value)> {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use std::convert::Infallible;

    let req = axum::http::Request::builder()
        .method("POST")
        .uri(&format!("/api/admin-chat/sessions/{session_id}/messages"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(json!({"content": "hello"}).to_string()))?;

    let resp = app.clone().oneshot(req).await?;
    let status = resp.status();
    let bytes = resp.into_body().collect().await?.to_bytes();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::String(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))
    };
    Ok((status, body))
}

/// Helper: GET messages from a session
async fn get_messages(app: &Router, token: &str, session_id: i64) -> anyhow::Result<(StatusCode, Value)> {
    get(app, &format!("/api/admin-chat/sessions/{session_id}/messages"), Some(token)).await
}

#[tokio::test]
async fn t164_admin_cannot_access_other_admin_session() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;

    // Create two non-super admins
    let admin_a = seed_admin(&pool, 2, 1, "test123").await?; // role=2 (System)
    let admin_b = seed_admin(&pool, 2, 1, "test456").await?;

    // Admin A creates a session
    let session_id = create_session(&app, &admin_a.token()?).await?;

    // Admin B tries to post messages to Admin A's session → 403
    let (status, body) = post_message(&app, &admin_b.token()?, session_id).await?;
    let code = body["code"].as_i64();
    assert!(
        status.is_client_error(),
        "admin B must be denied access to admin A's session; got {status} {body}"
    );
    assert_eq!(code, Some(2001), "expected error code 2001 (insufficient permission); got {body}");

    // Admin B tries to GET messages from Admin A's session → 403
    let (status, body) = get_messages(&app, &admin_b.token()?, session_id).await?;
    let code = body["code"].as_i64();
    assert!(
        status.is_client_error(),
        "admin B must be denied read access to admin A's session; got {status} {body}"
    );
    assert_eq!(code, Some(2001), "expected error code 2001 (insufficient permission); got {body}");

    Ok(())
}

#[tokio::test]
async fn t164_admin_can_access_own_session() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;

    let admin_a = seed_admin(&pool, 2, 1, "test123").await?;

    // Admin A creates a session
    let session_id = create_session(&app, &admin_a.token()?).await?;

    // Admin A can GET messages from own session (200, even if empty)
    let (status, body) = get_messages(&app, &admin_a.token()?, session_id).await?;
    assert_eq!(status, 200, "admin A must read own session; got {status} {body}");

    Ok(())
}

#[tokio::test]
async fn t164_super_can_access_other_admin_session() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;

    let admin_a = seed_admin(&pool, 2, 1, "test123").await?;
    let super_admin = seed_admin(&pool, 3, 1, "super123").await?; // role=3 (Super)

    // Admin A creates a session
    let session_id = create_session(&app, &admin_a.token()?).await?;

    // Super can read Admin A's session messages → 200
    let (status, body) = get_messages(&app, &super_admin.token()?, session_id).await?;
    assert_eq!(
        status, 200,
        "Super must be able to read any admin's session; got {status} {body}"
    );

    Ok(())
}

#[tokio::test]
async fn t164_non_super_cannot_access_deleted_admin_session() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;

    let admin_a = seed_admin(&pool, 2, 1, "test123").await?;
    let admin_b = seed_admin(&pool, 2, 1, "test456").await?;

    // Admin A creates a session
    let session_id = create_session(&app, &admin_a.token()?).await?;

    // Delete Admin A (sets session admin_id to NULL via ON DELETE SET NULL)
    delete_auth(&app, &format!("/api/admins/{}", admin_a.id), &admin_a.bearer()?).await?;

    // Admin B tries to read the now-orphaned session → 403 (only Super can access)
    let (status, body) = get_messages(&app, &admin_b.token()?, session_id).await?;
    assert!(
        status.is_client_error(),
        "non-Super must be denied access to orphaned session; got {status} {body}"
    );

    // Super can still access the orphaned session
    let super_admin = seed_admin(&pool, 3, 1, "super123").await?;
    let (status, body) = get_messages(&app, &super_admin.token()?, session_id).await?;
    assert_eq!(
        status, 200,
        "Super must access orphaned session; got {status} {body}"
    );

    Ok(())
}
