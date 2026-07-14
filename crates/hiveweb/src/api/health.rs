//! Process liveness and core dependency readiness endpoints.

use std::time::{Duration, Instant};

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use redis::Client as RedisClient;
use serde_json::{Value, json};
use sqlx::MySqlPool;

const CHECK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
struct HealthState {
    database: MySqlPool,
    redis: RedisClient,
}

/// Build the unauthenticated health router.
pub fn router(database: MySqlPool, redis: RedisClient) -> Router {
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .with_state(HealthState { database, redis })
}

async fn live() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn ready(State(state): State<HealthState>) -> (StatusCode, Json<Value>) {
    let started_at = Instant::now();
    let (database_result, redis_result) = tokio::join!(
        tokio::time::timeout(CHECK_TIMEOUT, check_database(&state.database)),
        tokio::time::timeout(CHECK_TIMEOUT, check_redis(&state.redis)),
    );

    let database_ready = matches!(database_result, Ok(Ok(())));
    let redis_ready = matches!(redis_result, Ok(Ok(())));
    let all_ready = database_ready && redis_ready;

    if !all_ready {
        tracing::warn!(
            operation = "health_ready",
            outcome = "not_ready",
            database_ready,
            redis_ready,
            duration_ms = started_at.elapsed().as_millis(),
            "core dependency readiness check failed"
        );
    }

    let status = if all_ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body_status = if all_ready { "ok" } else { "not_ready" };
    let check_status = |ready| if ready { "ok" } else { "error" };

    (
        status,
        Json(json!({
            "status": body_status,
            "checks": {
                "database": check_status(database_ready),
                "redis": check_status(redis_ready),
            }
        })),
    )
}

async fn check_database(database: &MySqlPool) -> Result<(), sqlx::Error> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(database)
        .await
        .map(|_| ())
}

async fn check_redis(redis: &RedisClient) -> redis::RedisResult<()> {
    let mut connection = redis.get_multiplexed_async_connection().await?;
    redis::cmd("PING")
        .query_async::<_, String>(&mut connection)
        .await
        .map(|_| ())
}
