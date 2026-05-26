pub mod auth;
pub mod admin;
pub mod dashboard;

// 004 Agent Runtime
pub mod agent;
pub mod capability;
pub mod category;
pub mod function;
pub mod plugin;
pub mod runtime;
pub mod skill;
pub mod chat;
pub mod tag;
pub mod tool;
pub mod workflow;

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
use crate::runtime::{
    CapabilityRegistry, InstancePool, Invoker, LlmRegistry, PoolConfig, RuntimeState,
    WorkflowExecutor,
};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct AppState {
    pub pool: MySqlPool,
    pub redis: RedisClient,
    pub s3: Client,
    /// 004 Agent Runtime — capability registry / instance pool / invoker / workflow / llm
    pub runtime_state: RuntimeState,
}

/// 启动期严格 12 步顺序（plan §Startup Initialization Order）：
///   1. env / dotenv —— 由 main.rs 完成（DATABASE_URL/JWT_SECRET 等 fail-fast）
///   2. DB migrations —— `cargo run --bin migrate`
///   3. capability registry upsert —— 启动期 INSERT ... ON DUPLICATE KEY UPDATE
///   4. builtin function upsert —— FR-010 v5 的 5 个 builtin（kind=1）
///   5. custom function 索引 —— 拉 DB 全部 kind=2，构 Arc<HashMap<identifier, FunctionDef>>
///   6. llm_presets.toml 加载 —— LlmRegistry::load_from_path()
///   7. ToolRegistry 装配 —— builtin + custom + workflow-wrap，注册到 agent::ToolRegistry
///   8. SubagentManager / MemoryStore 初始化
///   9. Instance Pool 空池
///  10. HTTP Router 装配（middleware → API group）
///  11. 后台任务启动（retention cron / pool idle reaper）
///  12. HTTP server listen
///
/// 当前 create_router 完成 9 + 10；3..8 + 11 在 main.rs 的 setup 阶段调用具体
/// services（Phase 3..7 实现后接入）。
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

    // ---- 004 Runtime state (steps 6+9; plan §Startup Initialization Order) ----
    let pool_inst = InstancePool::new(PoolConfig::from_env());
    let invoker = Arc::new(Invoker::new(pool_inst.clone()));
    let llm_path = std::env::var("LLM_PRESETS_PATH").unwrap_or_else(|_| "./llm_presets.toml".to_string());
    let llm = match LlmRegistry::load_from_path(&llm_path) {
        Ok(reg) => reg,
        Err(e) => {
            tracing::warn!(error = %e, "LlmRegistry load failed; using empty registry");
            Arc::new(LlmRegistry::new())
        }
    };
    let runtime_state = RuntimeState {
        capabilities: Arc::new(CapabilityRegistry::new()),
        pool: pool_inst,
        invoker,
        workflows: Arc::new(WorkflowExecutor::new()),
        llm,
    };

    let state = AppState { pool, redis, s3, runtime_state };

    // Rate-limit window is per-IP. Defaults: 100 req / 60 s.
    // Tune via env vars RATE_LIMIT_MAX and RATE_LIMIT_WINDOW_SECS.
    let rl_max: u64 = std::env::var("RATE_LIMIT_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let rl_window: u64 = std::env::var("RATE_LIMIT_WINDOW_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let rate_limit_state = RateLimitState::new(rl_max, Duration::from_secs(rl_window));

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
        .merge(plugin::router())
        .merge(function::router())
        .merge(tool::router())
        .merge(skill::router())
        .merge(category::router())
        .merge(tag::router())
        .merge(capability::router())
        .merge(runtime::router())
        .merge(agent::router())
        .merge(workflow::router())
        .merge(chat::router())
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
