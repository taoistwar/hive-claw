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
    /// Port to listen on (overrides HIVWEB_PORT env var)
    #[arg(long)]
    port: Option<u16>,
}

/// Try to initialise Langfuse from environment variables.
///
/// Reads:
/// - `LANGFUSE_ENABLED` / `LANGFUSE_PUBLIC_KEY` / `LANGFUSE_SECRET_KEY`
/// - `LANGFUSE_HOST` (defaults to `https://cloud.langfuse.com`)
///
/// If Langfuse is disabled or credentials are missing, tracing is silently skipped.
fn try_init_langfuse() {
    langfuse::lf_debug!("hiveweb::try_init_langfuse called, checking env vars");
    let enabled = std::env::var("LANGFUSE_ENABLED")
        .unwrap_or_default()
        .to_lowercase();

    // Auto-detect: if LANGFUSE_PUBLIC_KEY + SECRET_KEY are set but
    // LANGFUSE_ENABLED is missing, enable tracing automatically and warn.
    let has_public_key = std::env::var("LANGFUSE_PUBLIC_KEY")
        .map(|k| !k.is_empty())
        .unwrap_or(false);
    let has_secret_key = std::env::var("LANGFUSE_SECRET_KEY")
        .map(|k| !k.is_empty())
        .unwrap_or(false);

    langfuse::lf_debug!(
        "hiveweb::try_init_langfuse: enabled='{}' has_pk={} has_sk={}",
        enabled,
        has_public_key,
        has_secret_key
    );

    if enabled != "true" && enabled != "1" && enabled != "yes" {
        if has_public_key && has_secret_key {
            langfuse::lf_debug!(
                "hiveweb::try_init_langfuse: auto-detect enabled, keys present but LANGFUSE_ENABLED not set"
            );
            tracing::warn!(
                "LANGFUSE_ENABLED not set, but LANGFUSE_PUBLIC_KEY and SECRET_KEY are present. \
                 Enabling Langfuse tracing automatically. \
                 Set LANGFUSE_ENABLED=true explicitly to suppress this warning."
            );
        } else {
            langfuse::lf_debug!("hiveweb::try_init_langfuse: disabled (LANGFUSE_ENABLED not set)");
            tracing::info!("Langfuse tracing disabled (LANGFUSE_ENABLED not set)");
            return;
        }
    }

    let public_key = match std::env::var("LANGFUSE_PUBLIC_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            langfuse::lf_debug!("hiveweb::try_init_langfuse: LANGFUSE_PUBLIC_KEY missing");
            tracing::warn!("LANGFUSE_ENABLED=true but LANGFUSE_PUBLIC_KEY missing");
            return;
        }
    };
    let secret_key = match std::env::var("LANGFUSE_SECRET_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            langfuse::lf_debug!("hiveweb::try_init_langfuse: LANGFUSE_SECRET_KEY missing");
            tracing::warn!("LANGFUSE_ENABLED=true but LANGFUSE_SECRET_KEY missing");
            return;
        }
    };
    let host =
        std::env::var("LANGFUSE_HOST").unwrap_or_else(|_| "https://cloud.langfuse.com".to_string());

    langfuse::lf_debug!(
        "hiveweb::try_init_langfuse: creating config host={} pk_len={} sk_len={}",
        host,
        public_key.len(),
        secret_key.len()
    );
    let cfg = langfuse::LangfuseConfig {
        public_key,
        secret_key,
        host,
        environment: None,
        release: None,
        sample_rate: 1.0,
    };

    langfuse::lf_debug!("hiveweb::try_init_langfuse: calling LangfuseClient::new");
    match langfuse::LangfuseClient::new(cfg) {
        Some(client) => {
            langfuse::lf_debug!(
                "hiveweb::try_init_langfuse: client created, calling set_langfuse_client"
            );
            providers::set_langfuse_client(Some(std::sync::Arc::new(client)));
            langfuse::lf_debug!(
                "hiveweb::try_init_langfuse: Langfuse LLM tracing initialised successfully"
            );
            tracing::info!("Langfuse LLM tracing initialised");
        }
        None => {
            langfuse::lf_debug!("hiveweb::try_init_langfuse: LangfuseClient::new returned None!");
            tracing::warn!("LangfuseClient::new returned None, tracing disabled");
        }
    }
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

    // Initialise Langfuse LLM observability (non-blocking, async ingestion)
    try_init_langfuse();

    let host = std::env::var("HIVWEB_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = cli.port.unwrap_or_else(|| {
        std::env::var("HIVWEB_PORT")
            .unwrap_or_else(|_| "3000".to_string())
            .parse::<u16>()
            .expect("HIVWEB_PORT must be a valid number")
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

    // Create router
    let app = api::create_router(pool, redis, s3_client, ext_pool);
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
