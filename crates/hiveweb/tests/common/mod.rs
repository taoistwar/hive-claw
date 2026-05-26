//! Shared test harness for Phase 2.5 tests.
//!
//! Provides:
//!   - `test_app()` — builds the full axum router against the live MySQL/Redis/S3 infra
//!   - `mint_jwt(...)` — produces a Bearer token for a given admin id + role
//!   - `SeededAdmin` — RAII-style admin row that deletes itself on drop
//!
//! All helpers require `DATABASE_URL` (and optionally `REDIS_URL`). If absent,
//! the test fails with a clear "Phase 2.5 RED" message.

#![allow(dead_code)]

use anyhow::{anyhow, Result};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::MySqlPool;
use tower::ServiceExt;

pub use hiveweb::utils::jwt::create_token;

/// Build the test instance of the full axum router wired against the live
/// MySQL/Redis/S3 backends. Returns `Err` when required env vars are missing
/// so the calling test fails with a descriptive message.
pub async fn test_app() -> Result<Router> {
    let database_url = std::env::var("DATABASE_URL").map_err(|_| {
        anyhow!(
            "Phase 2.5 RED: DATABASE_URL not set. \
             Run `./scripts/dev-up.sh -d` and export DATABASE_URL."
        )
    })?;
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

    let pool = hiveweb::db::connection::create_pool(&database_url).await?;
    let redis = hiveweb::cache::redis::create_pool(&redis_url).await?;
    let s3 = hiveweb::storage::s3::create_client().await?;

    Ok(hiveweb::api::create_router(pool, redis, s3))
}

/// Open a direct MySQL pool from `DATABASE_URL` for seed / cleanup operations.
pub async fn test_pool() -> Result<MySqlPool> {
    let url = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow!("Phase 2.5 RED: DATABASE_URL not set"))?;
    Ok(MySqlPool::connect(&url).await?)
}

/// Mint a Bearer-style JWT for the given admin and role.
pub fn mint_jwt(admin_id: i64, role: i8) -> Result<String> {
    Ok(create_token(admin_id, role)?)
}

/// An admin row owned by a test. Deleted on drop (with a best-effort blocking
/// call) so tests stay isolated. Use the `seed_admin` constructor.
pub struct SeededAdmin {
    pub id: i64,
    pub phone: String,
    pub nickname: String,
    pub role: i8,
    pub status: i8,
    pool: MySqlPool,
}

impl SeededAdmin {
    pub fn bearer(&self) -> Result<String> {
        Ok(format!("Bearer {}", mint_jwt(self.id, self.role)?))
    }
    pub fn token(&self) -> Result<String> {
        mint_jwt(self.id, self.role)
    }
}

impl Drop for SeededAdmin {
    fn drop(&mut self) {
        // Best-effort cleanup. Tests should also call `cleanup()` explicitly
        // in async contexts; this Drop is the safety net.
        let pool = self.pool.clone();
        let id = self.id;
        tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::try_current().ok();
            if let Some(h) = handle {
                h.block_on(async move {
                    let _ = sqlx::query("DELETE FROM admins WHERE id = ?")
                        .bind(id)
                        .execute(&pool)
                        .await;
                });
            }
        });
    }
}

/// Insert a fresh admin row with a unique phone (random-suffix) and known
/// password. Returns the SeededAdmin handle.
///
/// `password` is bcrypt-hashed via the production helper so login tests can
/// authenticate as this admin too.
pub async fn seed_admin(
    pool: &MySqlPool,
    role: i8,
    status: i8,
    password: &str,
) -> Result<SeededAdmin> {
    let suffix: u32 = rand_suffix();
    let phone = format!("139{:08}", suffix);
    let nickname = format!("test-{}", suffix);
    let hash = hiveweb::utils::password::hash_password(password)
        .map_err(|e| anyhow!("hash_password failed: {e}"))?;

    let result = sqlx::query(
        r#"
        INSERT INTO admins (phone, nickname, password_hash, role, status, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, NOW(), NOW())
        "#,
    )
    .bind(&phone)
    .bind(&nickname)
    .bind(&hash)
    .bind(role)
    .bind(status)
    .execute(pool)
    .await?;
    let id = result.last_insert_id() as i64;

    Ok(SeededAdmin {
        id,
        phone,
        nickname,
        role,
        status,
        pool: pool.clone(),
    })
}

fn rand_suffix() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    nanos % 100_000_000
}

// ---------- HTTP helpers ----------

pub async fn post_json(app: &Router, path: &str, payload: Value) -> Result<(StatusCode, Value)> {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))?;
    send(app, req).await
}

pub async fn post_json_auth(
    app: &Router,
    path: &str,
    token: &str,
    payload: Value,
) -> Result<(StatusCode, Value)> {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(payload.to_string()))?;
    send(app, req).await
}

pub async fn delete_auth(app: &Router, path: &str, token: &str) -> Result<(StatusCode, Value)> {
    let req = Request::builder()
        .method("DELETE")
        .uri(path)
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())?;
    send(app, req).await
}

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
