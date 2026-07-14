use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use hiveweb::cache::redis::RedisClient;

fn unavailable_dependencies() -> (sqlx::MySqlPool, RedisClient) {
    let database = sqlx::MySqlPool::connect_lazy("mysql://health:health@127.0.0.1:1/health")
        .expect("lazy MySQL pool");
    let redis = redis::Client::open("redis://127.0.0.1:1").expect("lazy Redis client");
    (database, redis.into())
}

async fn get(path: &str) -> (StatusCode, Value) {
    let (database, redis) = unavailable_dependencies();
    let app = hiveweb::api::health::router(database, redis);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn live_is_independent_of_external_dependencies() {
    let (status, body) = get("/health/live").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"status": "ok"}));
}

#[tokio::test]
async fn ready_returns_service_unavailable_when_dependencies_are_down() {
    let (status, body) = get("/health/ready").await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body,
        serde_json::json!({
            "status": "not_ready",
            "checks": {
                "database": "error",
                "redis": "error"
            }
        })
    );
}
