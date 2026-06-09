//! Integration tests for the recommended games public API.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn app() -> impl std::future::Future<Output = Result<axum::Router, anyhow::Error>> {
    common::test_app()
}

async fn call_top(query: &str) -> (StatusCode, String) {
    let router = app().await.expect("test_app");
    let uri = format!("/api/recommended-games/top?{query}");
    let req = Request::builder()
        .uri(&uri)
        .method("GET")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&body).to_string();
    (status, text)
}

// ── T001: missing required params ──

#[tokio::test]
async fn missing_user_id_returns_error() {
    let (status, body) = call_top("channel=web&client_type=pc&client_version=1.0").await;
    assert!(status.is_client_error(), "expected 4xx, got {status}: {body}");
}

#[tokio::test]
async fn missing_channel_returns_error() {
    let (status, body) = call_top("user_id=123&client_type=pc&client_version=1.0").await;
    assert!(status.is_client_error(), "expected 4xx, got {status}: {body}");
}

#[tokio::test]
async fn missing_client_type_returns_error() {
    let (status, body) = call_top("user_id=123&channel=web&client_version=1.0").await;
    assert!(status.is_client_error(), "expected 4xx, got {status}: {body}");
}

#[tokio::test]
async fn missing_client_version_returns_error() {
    let (status, body) = call_top("user_id=123&channel=web&client_type=pc").await;
    assert!(status.is_client_error(), "expected 4xx, got {status}: {body}");
}

// ── T002: empty required params ──

#[tokio::test]
async fn empty_params_return_error() {
    let (status, body) = call_top("user_id=&channel=&client_type=&client_version=").await;
    assert!(status.is_client_error(), "expected 4xx, got {status}: {body}");
    assert!(body.contains("不能为空"), "expected non-empty error: {body}");
}

// ── T003: valid request ──

#[tokio::test]
async fn valid_request_returns_ok() {
    let (status, body) = call_top("user_id=test-user&channel=web&client_type=pc&client_version=1.0").await;
    assert_eq!(status, StatusCode::OK, "expected 200, got {status}: {body}");
    assert!(body.contains("\"data\""), "expected data field: {body}");
    println!(  "response body: {body}");
}

// ── T004: n param ──

#[tokio::test]
async fn n_param_is_optional() {
    let (status, _body) = call_top("user_id=u&channel=web&client_type=pc&client_version=1.0&n=5").await;
    assert_eq!(status, StatusCode::OK);
}
