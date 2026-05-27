//! SSE concurrency limit integration test (T163 / CHK232 / FR-027 v7)
//!
//! Verifies:
//!   1. CHAT_SSE_MAX_CONCURRENT_PER_ADMIN=2; starting 3 SSE streams → 3rd gets 429 + code 4291
//!   2. After closing one stream, the 3rd can succeed
//!
//! This test uses a custom SSE request builder (POST + JSON body + Accept: text/event-stream)
//! and reads just enough of the response to detect the status code before the stream hangs.

mod common;

use common::{mint_jwt, seed_admin, test_app};
use axum::body::Body;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// Build a POST /api/chat/sessions/:id/messages request with SSE Accept header.
fn sse_request(app: &Router, token: &str, session_id: i64) -> anyhow::Result<Request<Body>> {
    use axum::http::Request;
    Ok(Request::builder()
        .method("POST")
        .uri(&format!("/api/chat/sessions/{session_id}/messages"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "text/event-stream")
        .body(Body::from(json!({"content": "hello"}).to_string()))?)
}

/// Send a request and get just the status code quickly (without consuming the full SSE body).
async fn send_sse_status(app: &Router, req: Request<Body>) -> anyhow::Result<http::StatusCode> {
    // For SSE responses, we need to consume the response to get the status.
    // We only need the first few bytes to check if it's a JSON error (429) or SSE stream.
    let resp = app.clone().oneshot(req).await?;
    let status = resp.status();
    // If not 2xx, consume body to check error code
    if !status.is_success() {
        let bytes = resp.into_body().collect().await?.to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if let Some(code) = body.get("code").and_then(|v| v.as_u64()) {
            assert_eq!(code, 4291, "expected code 4291 for concurrency limit, got {code}");
        }
    }
    Ok(status)
}

/// Full SSE request that also reads the first event to confirm stream started.
async fn send_sse_full(app: &Router, req: Request<Body>) -> anyhow::Result<(http::StatusCode, Value)> {
    let resp = app.clone().oneshot(req).await?;
    let status = resp.status();
    let bytes = resp.into_body().collect().await?.to_bytes();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        // SSE responses may have multiple events; try to parse as JSON
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            // If not valid JSON, return as string for inspection
            Value::String(String::from_utf8_lossy(&bytes).into_owned())
        })
    };
    Ok((status, body))
}

use axum::http::Request;

#[tokio::test]
async fn sse_concurrency_limit_third_request_gets_429() -> anyhow::Result<()> {
    // Set limit to 2
    std::env::set_var("CHAT_SSE_MAX_CONCURRENT_PER_ADMIN", "2");

    let app = test_app().await?;
    let pool = common::test_pool().await?;
    let admin = seed_admin(&pool, 1, 1, "test123").await?;
    let token = admin.token()?;

    // Create a session
    let (status, body) = common::post_json_auth(
        &app,
        "/api/chat/sessions",
        &token,
        json!({"title": "sse-concurrency-test"}),
    )
    .await?;
    assert_eq!(status, http::StatusCode::OK, "create session: {status} {body}");
    let session_id = body["data"]["id"].as_i64().expect("missing session id");

    // Start 2 concurrent SSE streams (these should succeed — status 200)
    let req1 = sse_request(&app, &token, session_id)?;
    let req2 = sse_request(&app, &token, session_id)?;

    let (status1, status2) = tokio::join!(
        send_sse_full(&app, req1),
        send_sse_full(&app, req2),
    );

    // Both should be 200 OK (or possibly fail for other reasons, but not 429)
    let (s1, _b1) = status1?;
    let (s2, _b2) = status2?;
    assert_eq!(s1, http::StatusCode::OK, "first SSE stream should succeed");
    assert_eq!(s2, http::StatusCode::OK, "second SSE stream should succeed");

    // 3rd stream should get 429 (Too Many Requests)
    let req3 = sse_request(&app, &token, session_id)?;
    let (status3, body3) = send_sse_full(&app, req3).await?;
    assert_eq!(
        status3,
        http::StatusCode::TOO_MANY_REQUESTS,
        "3rd SSE stream should get 429, got {status3}: {body3}"
    );

    Ok(())
}

#[tokio::test]
async fn sse_concurrency_limit_releases_after_close() -> anyhow::Result<()> {
    std::env::set_var("CHAT_SSE_MAX_CONCURRENT_PER_ADMIN", "2");

    let app = test_app().await?;
    let pool = common::test_pool().await?;
    let admin = seed_admin(&pool, 1, 1, "test123").await?;
    let token = admin.token()?;

    // Create a session
    let (status, body) = common::post_json_auth(
        &app,
        "/api/chat/sessions",
        &token,
        json!({"title": "sse-release-test"}),
    )
    .await?;
    assert_eq!(status, http::StatusCode::OK);
    let session_id = body["data"]["id"].as_i64().expect("missing session id");

    // Start 2 streams
    let req1 = sse_request(&app, &token, session_id)?;
    let req2 = sse_request(&app, &token, session_id)?;

    let (s1, _b1) = send_sse_full(&app, req1).await?;
    let (s2, _b2) = send_sse_full(&app, req2).await?;
    assert_eq!(s1, http::StatusCode::OK);
    assert_eq!(s2, http::StatusCode::OK);

    // The guard drop happens when the response body is fully consumed.
    // After consuming responses 1 and 2, the concurrency slots should be released.
    // Wait briefly for the async drop to complete
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Now a 3rd stream should succeed (slots released)
    let req3 = sse_request(&app, &token, session_id)?;
    let (status3, _body3) = send_sse_full(&app, req3).await?;
    assert_eq!(
        status3,
        http::StatusCode::OK,
        "3rd SSE stream should succeed after previous ones closed"
    );

    Ok(())
}

#[tokio::test]
async fn sse_concurrency_per_admin_isolation() -> anyhow::Result<()> {
    std::env::set_var("CHAT_SSE_MAX_CONCURRENT_PER_ADMIN", "2");

    let app = test_app().await?;
    let pool = common::test_pool().await?;

    // Create two different admins
    let admin_a = seed_admin(&pool, 1, 1, "test123").await?;
    let admin_b = seed_admin(&pool, 1, 1, "test456").await?;
    let token_a = admin_a.token()?;
    let token_b = admin_b.token()?;

    // Each admin creates their own session
    let (_, body_a) = common::post_json_auth(
        &app,
        "/api/chat/sessions",
        &token_a,
        json!({"title": "admin-a-session"}),
    )
    .await?;
    let session_a = body_a["data"]["id"].as_i64().expect("missing session id");

    let (_, body_b) = common::post_json_auth(
        &app,
        "/api/chat/sessions",
        &token_b,
        json!({"title": "admin-b-session"}),
    )
    .await?;
    let session_b = body_b["data"]["id"].as_i64().expect("missing session id");

    // Admin A starts 2 streams (hits their limit)
    let req_a1 = sse_request(&app, &token_a, session_a)?;
    let req_a2 = sse_request(&app, &token_a, session_a)?;
    let (s_a1, _) = send_sse_full(&app, req_a1).await?;
    let (s_a2, _) = send_sse_full(&app, req_a2).await?;
    assert_eq!(s_a1, http::StatusCode::OK);
    assert_eq!(s_a2, http::StatusCode::OK);

    // Admin B should still be able to start 2 streams (isolated counter)
    let req_b1 = sse_request(&app, &token_b, session_b)?;
    let req_b2 = sse_request(&app, &token_b, session_b)?;
    let (s_b1, _) = send_sse_full(&app, req_b1).await?;
    let (s_b2, _) = send_sse_full(&app, req_b2).await?;
    assert_eq!(s_b1, http::StatusCode::OK);
    assert_eq!(s_b2, http::StatusCode::OK);

    // Admin B's 3rd should fail (their own limit)
    let req_b3 = sse_request(&app, &token_b, session_b)?;
    let (s_b3, _) = send_sse_full(&app, req_b3).await?;
    assert_eq!(s_b3, http::StatusCode::TOO_MANY_REQUESTS);

    Ok(())
}
