pub mod admin;
pub mod admin_audit_log;
pub mod auth;
pub mod dashboard;
pub mod login_record;

// 004 Agent Runtime
pub mod agent;
pub mod agent_hook;
pub mod capability;
pub mod category;
pub mod chat_assistant;
pub mod chat_common;
pub mod chat_messages;
pub mod function;
pub mod game;
pub mod global_config;
pub mod health;
pub mod newsession;
pub mod plugin;
pub mod recommended_game;
pub mod runtime;
pub mod sensitive_word;
pub mod skill;
pub mod tag;
pub mod tool;
pub mod user;
pub mod users;
pub mod workflow;

use aws_sdk_s3::Client;
use axum::{Router, http::HeaderValue, http::StatusCode, middleware};
use sqlx::MySqlPool;
use tower_http::{
    cors::CorsLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use crate::cache::redis::RedisClient;
use crate::middleware::auth::admin_auth_middleware;
use crate::middleware::rate_limit::{RateLimitState, rate_limit_middleware};
use crate::middleware::request_body_log::log_request_body_middleware;
use crate::middleware::request_id::request_id_middleware;
use crate::runtime::{
    CapabilityRegistry, InstancePool, Invoker, LlmRegistry, PoolConfig, RuntimeState,
    WorkflowExecutor,
};
use crate::utils::error::{ApiResponse, AppError};
use std::sync::Arc;
use std::time::Duration;

/// 插件系统已关闭时返回 `PluginSystemDisabled` 响应。
/// 供 Plugin 上传/下载/调用等入口统一使用，避免散落 503 文案。
pub fn require_s3(state: &AppState) -> Result<&aws_sdk_s3::Client, ApiResponse<()>> {
    state.s3.as_ref().ok_or_else(|| {
        AppError::PluginSystemDisabled(
            "插件系统已关闭 (PLUGIN_SYSTEM_ENABLED=false)，此功能不可用".into(),
        )
        .into_response()
    })
}

#[derive(Clone)]
pub struct AppState {
    pub pool: MySqlPool,
    pub redis: RedisClient,
    /// S3 客户端。仅在 `PLUGIN_SYSTEM_ENABLED=true` 时为 `Some`。
    /// 所有 Plugin 上传/下载/调用入口及 `s3.*` capability 都应检查 `s3.is_some()`。
    pub s3: Option<Client>,
    /// 004 Agent Runtime — capability registry / instance pool / invoker / workflow / llm
    pub runtime_state: RuntimeState,
    /// 外部只读数据库连接（assistant API 用户校验等）
    pub ext_pool: Option<MySqlPool>,
    /// 010 Sensitive Word Filter — in-memory filter engine
    pub sensitive_filter: crate::services::sensitive_filter::SensitiveFilter,
}

/// DB migration 是服务启动外的前置步骤：生产环境由部署流程预建表，开发/测试环境
/// 必须先运行 `cargo run -p hiveweb --bin migrate`。主服务不会自动建表或迁移。
///
/// 启动期严格 11 步顺序（plan §Startup Initialization Order）：
///   1. env / dotenv —— 由 main.rs 完成（DATABASE_URL/JWT_SECRET 等 fail-fast）
///   2. capability registry upsert —— 启动期 INSERT ... ON DUPLICATE KEY UPDATE
///   3. builtin function upsert —— FR-010 v5 的 5 个 builtin（kind=1）
///   4. custom function 索引 —— 拉 DB 全部 kind=2，构 Arc<HashMap<identifier, FunctionDef>>
///   5. llm_presets.toml 加载 —— LlmRegistry::load_from_path()
///   6. ToolRegistry 装配 —— builtin + custom + workflow-wrap，注册到 agent::ToolRegistry
///   7. SubagentManager / MemoryStore 初始化
///   8. Instance Pool 空池
///   9. HTTP Router 装配（middleware → API group）
///  10. 后台任务启动（retention cron / pool idle reaper）
///  11. HTTP server listen
///
/// 当前 create_router 完成 8 + 9；2..7 + 10 在 main.rs 的 setup 阶段调用具体
/// services（Phase 3..7 实现后接入）。
pub fn create_router(
    pool: MySqlPool,
    redis: RedisClient,
    s3: Option<Client>,
    ext_pool: Option<MySqlPool>,
    sensitive_filter: crate::services::sensitive_filter::SensitiveFilter,
) -> Router {
    let health_routes = health::router(pool.clone(), redis.clone());
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
    let llm_path =
        std::env::var("LLM_PRESETS_PATH").unwrap_or_else(|_| "./llm_presets.toml".to_string());
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

    let state = AppState {
        pool,
        redis,
        s3: s3.clone(),
        runtime_state,
        ext_pool,
        sensitive_filter,
    };

    // Rate-limit window is per-IP. Defaults: 180 req / 60 s.
    // Tune via env vars RATE_LIMIT_MAX and RATE_LIMIT_WINDOW_SECS.
    let rl_max: u64 = std::env::var("RATE_LIMIT_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(180);
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
        .merge(auth::router_public())
        .merge(recommended_game::router_public())
        .merge(chat_assistant::router())
        .merge(chat_messages::router())
        .merge(newsession::router());

    let admin_protected_routes = Router::new()
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
        .merge(admin_audit_log::router())
        .merge(login_record::router())
        .merge(agent::router())
        .merge(agent_hook::router())
        .merge(workflow::router())
        .merge(user::router())
        .merge(recommended_game::router())
        .merge(global_config::router())
        .merge(game::router())
        .merge(sensitive_word::router())
        .layer(middleware::from_fn(admin_auth_middleware))
        .layer(middleware::from_fn_with_state(
            rate_limit_state,
            rate_limit_middleware,
        ));

    let api_routes = Router::new()
        .merge(public_routes)
        .merge(admin_protected_routes)
        .fallback(|| async { StatusCode::NOT_FOUND })
        .with_state(state);

    Router::new()
        .merge(health_routes)
        .nest("/api", api_routes)
        .layer(cors)
        .layer(tracing_layer)
        .layer(axum::middleware::from_fn(log_request_body_middleware))
        // request_id is the outermost layer so every other layer (cors,
        // tracing, rate-limit, auth, handlers) sees the same id.
        .layer(axum::middleware::from_fn(request_id_middleware))
}
