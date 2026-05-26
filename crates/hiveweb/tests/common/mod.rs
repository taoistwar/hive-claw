//! Shared test harness for Phase 2.5 red-phase tests.
//!
//! These tests are intentionally written to fail until either:
//!   1. The local infra is brought up (`scripts/dev-up.sh -d`), and
//!   2. The features they assert (T092/T093/T096/T097, etc.) are implemented.
//!
//! Until then, `test_app()` returns an `Err` that causes the test to be
//! reported as a failure with a descriptive message — that is the
//! intended RED signal.

#![allow(dead_code)]

use anyhow::{anyhow, Result};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

/// Build a test instance of the full axum router wired against the real
/// MySQL, Redis, and S3-compatible backends. Returns `Err` when any of the
/// required env vars are missing, so the calling test fails with a clear
/// message.
pub async fn test_app() -> Result<Router> {
    let database_url = std::env::var("DATABASE_URL").map_err(|_| {
        anyhow!(
            "Phase 2.5 RED: DATABASE_URL not set. \
             Run `./scripts/dev-up.sh -d` and re-export DATABASE_URL."
        )
    })?;
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

    let pool = hiveweb::db::connection::create_pool(&database_url).await?;
    let redis = hiveweb::cache::redis::create_pool(&redis_url).await?;
    let s3 = hiveweb::storage::s3::create_client().await?;

    Ok(hiveweb::api::create_router(pool, redis, s3))
}

/// Issue a JSON POST and return (status, body json).
pub async fn post_json(app: &Router, path: &str, payload: Value) -> Result<(StatusCode, Value)> {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))?;
    send(app, req).await
}

/// Issue a GET with an optional bearer token.
pub async fn get(app: &Router, path: &str, token: Option<&str>) -> Result<(StatusCode, Value)> {
    let mut builder = Request::builder().method("GET").uri(path);
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {}", t));
    }
    let req = builder.body(Body::empty())?;
    send(app, req).await
}

async fn send(app: &Router, req: Request<Body>) -> Result<(StatusCode, Value)> {
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
