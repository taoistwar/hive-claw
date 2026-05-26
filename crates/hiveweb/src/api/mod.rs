pub mod auth;
pub mod admin;
pub mod dashboard;

use axum::{
    http::HeaderValue,
    middleware,
    Router,
};
use aws_sdk_s3::Client;
use redis::Client as RedisClient;
use sqlx::MySqlPool;
use tower_http::{
    cors::CorsLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use crate::middleware::auth::auth_middleware;
use crate::middleware::rate_limit::{rate_limit_middleware, RateLimitState};
use crate::middleware::request_id::request_id_middleware;
use std::time::Duration;

#[derive(Clone)]
pub struct AppState {
    pub pool: MySqlPool,
    pub redis: RedisClient,
    pub s3: Client,
}

pub fn create_router(pool: MySqlPool, redis: RedisClient, s3: Client) -> Router {
    let allowed_origins = std::env::var("CORS_ALLOWED_ORIGINS").unwrap_or_else(|_| "*".to_string());

    let cors = if allowed_origins == "*" {
        CorsLayer::very_permissive()
    } else {
        let origins: Vec<HeaderValue> = allowed_origins
            .split(',')
            .map(|origin| origin.trim().parse())
            .filter_map(Result::ok)
            .collect();
        CorsLayer::new()
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any)
            .allow_credentials(true)
            .allow_origin(origins)
    };

    let state = AppState { pool, redis, s3 };

    let rate_limit_state = RateLimitState::new(100, Duration::from_secs(60));

    let tracing_layer = TraceLayer::new_for_http()
        .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
        .on_request(DefaultOnRequest::new().level(Level::INFO))
        .on_response(DefaultOnResponse::new().level(Level::INFO));

    let public_routes = Router::new()
        .merge(auth::router_public());

    let protected_routes = Router::new()
        .merge(auth::router_protected())
        .merge(admin::router())
        .merge(dashboard::router())
        .layer(middleware::from_fn(auth_middleware))
        .layer(middleware::from_fn_with_state(rate_limit_state, rate_limit_middleware));

    let api_routes = Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .with_state(state);

    Router::new()
        .nest("/api", api_routes)
        .layer(cors)
        .layer(tracing_layer)
        // request_id is the outermost layer so every other layer (cors,
        // tracing, rate-limit, auth, handlers) sees the same id.
        .layer(axum::middleware::from_fn(request_id_middleware))
}
