use tracing_subscriber::{self, EnvFilter};

mod api;
mod cache;
mod db;
mod middleware;
mod models;
mod services;
mod storage;
mod utils;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("hiveweb=debug".parse()?)
        )
        .json()
        .init();

    // Load environment variables
    dotenvy::dotenv()?;

    let host = std::env::var("HIVWEB_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("HIVWEB_PORT")
        .unwrap_or_else(|_| "3000".to_string())
        .parse::<u16>()?;

    // Initialize database connection pool
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set");
    let pool = db::connection::create_pool(&database_url).await?;

    // Initialize Redis connection
    let redis_url = std::env::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let redis = cache::redis::create_pool(&redis_url).await?;

    // Initialize S3 client
    let s3_client = storage::s3::create_client().await?;

    // Create router
    let app = api::create_router(pool, redis, s3_client);

    // Start server
    let addr = format!("{}:{}", host, port);
    tracing::info!("Starting server on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
