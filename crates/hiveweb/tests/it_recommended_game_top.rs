//! Integration tests for the recommended games public API.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn compute_sign(body: &str) -> String {
    let secret = hiveweb::api::chat_common::get_assistant_secret();
    if secret.is_empty() {
        return String::new();
    }
    let sign_string = format!("{}/api/recommended-games/top?body={}", secret, body);
    format!("{:x}", md5::compute(sign_string.as_bytes()))
}

fn app() -> impl std::future::Future<Output = Result<axum::Router, anyhow::Error>> {
    common::test_app()
}

async fn call_top(body_json: &str) -> (StatusCode, String) {
    let router = app().await.expect("test_app");
    let sign = compute_sign(body_json);
    let uri = if sign.is_empty() {
        "/api/recommended-games/top".to_string()
    } else {
        format!("/api/recommended-games/top?sign={sign}")
    };
    let req = Request::builder()
        .uri(&uri)
        .method("POST")
        .header("Content-Type", "application/json; charset=UTF-8")
        .body(Body::from(body_json.to_string()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&body).to_string();
    (status, text)
}

fn body(user_id: &str, channel: &str, client_type: &str, client_version: &str) -> String {
    format!(
        r#"{{"user_id":"{}","channel":"{}","client_type":"{}","client_version":"{}"}}"#,
        user_id, channel, client_type, client_version
    )
}

// ── T001: missing fields ──

#[tokio::test]
async fn missing_user_id_returns_error() {
    let b = r#"{"channel":"web","client_type":"pc","client_version":"1.0"}"#;
    let (status, body) = call_top(b).await;
    assert!(
        status.is_client_error(),
        "expected 4xx, got {status}: {body}"
    );
}

#[tokio::test]
async fn missing_channel_returns_error() {
    let b = r#"{"user_id":"123","client_type":"pc","client_version":"1.0"}"#;
    let (status, body) = call_top(b).await;
    assert!(
        status.is_client_error(),
        "expected 4xx, got {status}: {body}"
    );
}

#[tokio::test]
async fn missing_client_type_returns_error() {
    let b = r#"{"user_id":"123","channel":"web","client_version":"1.0"}"#;
    let (status, body) = call_top(b).await;
    assert!(
        status.is_client_error(),
        "expected 4xx, got {status}: {body}"
    );
}

#[tokio::test]
async fn missing_client_version_returns_error() {
    let b = r#"{"user_id":"123","channel":"web","client_type":"pc"}"#;
    let (status, body) = call_top(b).await;
    assert!(
        status.is_client_error(),
        "expected 4xx, got {status}: {body}"
    );
}

// ── T002: empty required params ──

#[tokio::test]
async fn empty_params_return_error() {
    let (status, body) = call_top(&body("", "", "", "")).await;
    assert!(
        status.is_client_error(),
        "expected 4xx, got {status}: {body}"
    );
    assert!(
        body.contains("不能为空"),
        "expected non-empty error: {body}"
    );
}

// ── T003: valid request ──

#[tokio::test]
async fn valid_request_returns_ok() {
    let (status, body) = call_top(&body("test-user", "web", "pc", "1.0")).await;
    assert_eq!(status, StatusCode::OK, "expected 200, got {status}: {body}");
    assert!(body.contains("\"data\""), "expected data field: {body}");
}
