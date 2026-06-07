use clap::Parser;
use tracing_subscriber::{self, EnvFilter};

mod api;
mod cache;
mod db;
mod middleware;
mod models;
mod runtime;
mod services;
mod storage;
mod utils;

#[derive(Parser)]
#[command(name = "hiveweb", about = "HiveClaw Admin Center")]
struct Cli {
    /// Port to listen on (overrides HIVEWEB_PORT env var)
    #[arg(long)]
    port: Option<u16>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Load .env file if it exists, but don't fail if it doesn't
    match dotenvy::dotenv_override() {
        Ok(path) => {
            println!("[ENV] Loaded .env from: {}", path.display());
        }
        Err(_) => {
            println!("[ENV] No .env file found, using environment variables");
        }
    }

    // Initialize logging
    let is_dev = std::env::var("APP_ENV")
        .map(|v| v == "development" || v == "dev")
        .unwrap_or(true);

    let env_filter = if is_dev {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("debug"))
            .add_directive("hiveweb=debug".parse().unwrap())
    } else {
        EnvFilter::from_default_env().add_directive("hiveweb=info".parse()?)
    };

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .json()
        .init();

    tracing::info!("=== HiveClaw Admin Center Starting ===");

    let host = std::env::var("HIVEWEB_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = cli.port.unwrap_or_else(|| {
        std::env::var("HIVEWEB_PORT")
            .unwrap_or_else(|_| "3300".to_string())
            .parse::<u16>()
            .expect("HIVEWEB_PORT must be a valid number")
    });
    println!("Host: {}, Port: {}", host, port);

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

    // Initialize S3 client
    let s3_client = storage::s3::create_client().await?;
    tracing::info!("S3 storage client initialized");

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
    tracing::info!("Server listening on http://{}", addr);
    tracing::info!("=== HiveClaw Admin Center Ready ===");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

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
