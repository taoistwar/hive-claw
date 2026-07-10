use clap::Parser;

use tracing_subscriber::{
    self, EnvFilter,
    fmt::{self, time::OffsetTime},
    layer::SubscriberExt,
};

mod api;
mod app_mode;
mod cache;
mod db;
mod middleware;
mod models;
mod runtime;
mod services;
mod storage;
mod utils;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Load .env file if it exists, but don't fail if it doesn't
    match dotenvy::dotenv_override() {
        Ok(path) => {
            if !app_mode::init(cli.mode.as_deref()).is_production() {
                println!("[ENV] Loaded .env from: {}", path.display());
            }
        }
        Err(_) => {
            if !app_mode::init(cli.mode.as_deref()).is_production() {
                println!("[ENV] No .env file found, using environment variables");
            }
        }
    }

    // Initialize logging
    match app_mode::get() {
        app_mode::AppMode::Production => file_tracing()?,
        app_mode::AppMode::Development => console_tracing()?,
        app_mode::AppMode::Test => test_tracing()?,
    }

    let host = std::env::var("HIVEWEB_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = cli.port.unwrap_or_else(|| {
        std::env::var("HIVEWEB_PORT")
            .unwrap_or_else(|_| "3300".to_string())
            .parse::<u16>()
            .expect("HIVEWEB_PORT must be a valid number")
    });
    tracing::info!("Server will run at http://{}:{}", host, port);

    // Initialize database connection pool
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = db::connection::create_pool(&database_url).await?;

    // Mask password from database URL for logging
    let db_url_display = mask_url_password(&database_url);
    tracing::info!("Database initialized: {}", db_url_display);

    // Initialize Redis connection
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let redis = cache::redis::create_pool(&redis_url).await?;
    tracing::info!("Redis initialized: {}", mask_url_password(&redis_url));

    // Initialize S3 client (only when plugin system is enabled)
    let s3_client = if plugin_system_enabled() {
        let c: aws_sdk_s3::Client = storage::s3::create_client().await?;
        tracing::info!("S3 storage client initialized (plugin system enabled)");
        Some(c)
    } else {
        tracing::warn!(
            "PLUGIN_SYSTEM_ENABLED=false; S3 client skipped. \
             Plugin upload/download/invoke and s3.* capabilities are disabled."
        );
        None
    };

    // Initialize external read-only database for assistant API
    let ext_pool = match std::env::var("EXTERNAL_DB_URL") {
        Ok(url) if !url.is_empty() => match db::connection::create_pool(&url).await {
            Ok(p) => {
                tracing::info!("External DB initialized: {}", mask_url_password(&url));
                Some(p)
            }
            Err(e) => {
                tracing::warn!(
                    "External DB connection failed ({}), assistant API will be unavailable",
                    e
                );
                None
            }
        },
        _ => {
            tracing::info!("EXTERNAL_DB_URL not set, assistant API will be unavailable");
            None
        }
    };

    // Startup step 4 (plan §Startup Initialization Order): builtin function upsert
    if let Err(e) = runtime::builtins::ensure_registered(&pool).await {
        // 启动期 builtin upsert 失败 → panic（schema 错乱比启动失败更严重）
        panic!("builtin functions upsert failed: {e}");
    }

    // Startup: initialize sensitive word filter
    let sensitive_filter = crate::services::sensitive_filter::SensitiveFilter::new();
    if let Err(e) = sensitive_filter.load_from_db(&pool).await {
        tracing::warn!(error = %e, "Failed to load sensitive words from DB, filter disabled");
    }

    // Create router
    let app = api::create_router(pool, redis, s3_client, ext_pool, sensitive_filter);
    tracing::info!("HTTP router initialized with CORS and rate limiting");

    // Start server
    let addr = format!("{}:{}", host, port);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(Parser)]
#[command(name = "hiveweb", about = "HiveClaw Admin Center")]
struct Cli {
    /// Port to listen on (overrides HIVEWEB_PORT env var)
    #[arg(long)]
    port: Option<u16>,
    /// 运行模式: prod | dev | test（优先级高于 APP_ENV 环境变量）
    #[arg(long)]
    mode: Option<String>,
}

pub fn file_tracing() -> anyhow::Result<()> {
    // 可以配置日志的自动滚动周期，如下配置表示日志文件保存在logs目录下，日志名称为app.log+时间
    let log_dir = std::env::var("LOG_DIR").unwrap_or_else(|_| "logs".to_string());
    std::fs::create_dir_all(&log_dir).ok();
    let file_appender = tracing_appender::rolling::daily(log_dir, "hiveweb.log");
    // guard 这是一个守卫，生命周期需要贯穿整个主进程，所以我们在最后将他作为返回参数返回
    let (non_blocking_appender, guard) = tracing_appender::non_blocking(file_appender);
    // 将 guard 泄漏，使其在进程生命周期内保持有效
    std::mem::forget(guard);
    // fmt — JSON 格式输出到日志文件
    let fmt_layer = fmt::layer()
        .json()
        .with_target(true)
        .with_level(true)
        .with_ansi(false)
        .with_timer(fmt::time::SystemTime::default())
        .with_writer(non_blocking_appender);
    // subscriber
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive("hiveweb=info".parse()?);
    let subscriber = tracing_subscriber::registry::Registry::default()
        .with(fmt_layer)
        .with(env_filter);
    // global
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(())
}
pub fn console_tracing() -> anyhow::Result<()> {
    use tracing_subscriber::{
        self, EnvFilter, filter::filter_fn, fmt::time::OffsetTime, prelude::*,
    };

    use hiveweb::sqlx::sqlx_layer::SqlxLayer;
    use time::{UtcOffset, macros::format_description};
    // 配置日志时间格式，配置时区为东8区，
    let offset = UtcOffset::from_hms(8, 0, 0).unwrap_or(UtcOffset::UTC);
    // 时间格式为  年-月-日 时:分:秒 格式
    let logger_time = OffsetTime::new(
        offset,
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second]"),
    );
    // dev 模式直接使用 debug 级别，忽略 RUST_LOG
    let env_filter =
        EnvFilter::new("sqlx::query=debug,sqlx::formatted_query=debug,hiveweb=debug,info");

    let sqlx_layer = SqlxLayer::new();

    let fmt_layer = tracing_subscriber::fmt::layer()
        .pretty()
        .with_ansi(true)
        .with_file(true)
        .with_writer(std::io::stdout)
        .with_timer(logger_time)
        .with_target(true)
        .with_level(true)
        .with_filter(filter_fn(|metadata| metadata.target() != "sqlx::query"));
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(sqlx_layer)
        .init();

    Ok(())
}

/// Test mode: compact console output plus daily JSON file output.
pub fn test_tracing() -> anyhow::Result<()> {
    use tracing_subscriber::{filter::filter_fn, prelude::*};

    use hiveweb::sqlx::sqlx_layer::SqlxLayer;
    use time::{UtcOffset, macros::format_description};

    let log_dir = std::env::var("LOG_DIR").unwrap_or_else(|_| "logs".to_string());
    std::fs::create_dir_all(&log_dir)?;
    let file_appender = tracing_appender::rolling::daily(log_dir, "hiveweb.log");
    let (non_blocking_appender, guard) = tracing_appender::non_blocking(file_appender);
    std::mem::forget(guard);

    let file_layer = fmt::layer()
        .json()
        .with_target(true)
        .with_level(true)
        .with_ansi(false)
        .with_timer(fmt::time::SystemTime::default())
        .with_writer(non_blocking_appender);

    let offset = UtcOffset::from_hms(8, 0, 0).unwrap_or(UtcOffset::UTC);
    let logger_time = OffsetTime::new(
        offset,
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second]"),
    );
    let console_layer = fmt::layer()
        .with_ansi(true)
        .with_file(true)
        .with_writer(std::io::stdout)
        .with_timer(logger_time)
        .with_target(true)
        .with_level(true)
        .with_filter(filter_fn(|metadata| metadata.target() != "sqlx::query"));

    let env_filter =
        EnvFilter::new("sqlx::query=debug,sqlx::formatted_query=debug,hiveweb=debug,info");
    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(console_layer)
        .with(SqlxLayer::new())
        .init();

    Ok(())
}

fn mask_url_password(url: &str) -> String {
    if let Some(at_pos) = url.find('@') {
        let after_protocol = url.find("://").map(|p| p + 3).unwrap_or(0);
        let host_part = &url[after_protocol..at_pos];
        if let Some(colon_pos) = host_part.find(':') {
            let username = &host_part[..colon_pos];
            let rest = &url[at_pos..];
            let prefix = &url[..after_protocol];
            return format!("{}{}:***{}", prefix, username, rest);
        }
    }
    url.to_string()
}

/// 读取 `PLUGIN_SYSTEM_ENABLED`（默认 `true`）。
/// 关闭后跳过 S3 客户端初始化，Plugin 上传/下载/调用及 s3.* capability 全部不可用。
pub fn plugin_system_enabled() -> bool {
    match std::env::var("PLUGIN_SYSTEM_ENABLED") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off" | ""
        ),
        Err(_) => true,
    }
}
