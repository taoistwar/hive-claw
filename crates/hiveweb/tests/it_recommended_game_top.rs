//! Integration tests for the legacy recommended-games public API.

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

fn app(ext_pool: Option<sqlx::MySqlPool>) -> axum::Router {
    let database =
        sqlx::MySqlPool::connect_lazy("mysql://offline:offline@127.0.0.1:1/offline_test")
            .expect("lazy MySQL pool");
    let redis = redis::Client::open("redis://127.0.0.1:1").expect("lazy Redis client");
    hiveweb::api::create_router(
        database,
        redis.into(),
        None,
        ext_pool,
        hiveweb::services::sensitive_filter::SensitiveFilter::new(),
        std::sync::Arc::new(hiveweb::runtime::LlmRegistry::new()),
    )
}

async fn call_top(router: axum::Router, body_json: &str) -> (StatusCode, String) {
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
    let (status, body) = call_top(app(None), b).await;
    assert!(
        status.is_client_error(),
        "expected 4xx, got {status}: {body}"
    );
}

#[tokio::test]
async fn missing_channel_returns_error() {
    let b = r#"{"user_id":"123","client_type":"pc","client_version":"1.0"}"#;
    let (status, body) = call_top(app(None), b).await;
    assert!(
        status.is_client_error(),
        "expected 4xx, got {status}: {body}"
    );
}

#[tokio::test]
async fn missing_client_type_returns_error() {
    let b = r#"{"user_id":"123","channel":"web","client_version":"1.0"}"#;
    let (status, body) = call_top(app(None), b).await;
    assert!(
        status.is_client_error(),
        "expected 4xx, got {status}: {body}"
    );
}

#[tokio::test]
async fn missing_client_version_returns_error() {
    let b = r#"{"user_id":"123","channel":"web","client_type":"pc"}"#;
    let (status, body) = call_top(app(None), b).await;
    assert!(
        status.is_client_error(),
        "expected 4xx, got {status}: {body}"
    );
}

// ── T002: empty required params ──

#[tokio::test]
async fn empty_params_return_error() {
    let (status, body) = call_top(app(None), &body("", "", "", "")).await;
    assert!(
        status.is_client_error(),
        "expected 4xx, got {status}: {body}"
    );
    assert!(
        body.contains("不能为空"),
        "expected non-empty error: {body}"
    );
}

// ── T003: explicit external-DB success smoke ──

#[tokio::test]
#[ignore = "requires crates/hiveweb/.env EXTERNAL_DB_URL and compatible read-only data"]
async fn valid_request_returns_ok_with_external_db() -> anyhow::Result<()> {
    let env_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    dotenvy::from_path(env_path).ok();
    let url = std::env::var("EXTERNAL_DB_URL")
        .map_err(|_| anyhow::anyhow!("EXTERNAL_DB_URL is required for this ignored smoke test"))?;
    let ext_pool = sqlx::MySqlPool::connect(&url).await?;

    let eligible: Option<(String, String)> = sqlx::query_as(
        r#"SELECT DISTINCT pc.prom_channel, wide.client_type
           FROM recommended_games recommended
           INNER JOIN cc_logic_game game
             ON recommended.game_id = game.id AND game.status = 1
           INNER JOIN cc_logic_game_wide wide
             ON recommended.game_id = wide.logic_game_id
           INNER JOIN cc_promotion_channel pc
             ON wide.channel_game_tag = pc.game_tag
           WHERE recommended.tag IN ('运营推荐', '新游上线', '本周热玩')
           LIMIT 1"#,
    )
    .fetch_optional(&ext_pool)
    .await?;
    let (channel, client_type) = eligible.ok_or_else(|| {
        anyhow::anyhow!("external DB has no eligible recommended-game channel/client_type pair")
    })?;

    let request_body = body("test-user", &channel, &client_type, "1.0");
    let (status, response_body) = call_top(app(Some(ext_pool)), &request_body).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "expected 200, got {status}: {response_body}"
    );
    let response: serde_json::Value = serde_json::from_str(&response_body)?;
    assert!(
        response["data"].is_array(),
        "expected data array: {response_body}"
    );
    Ok(())
}
