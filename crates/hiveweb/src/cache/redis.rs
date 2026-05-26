use redis::Client;

pub type RedisClient = Client;

pub async fn create_pool(redis_url: &str) -> anyhow::Result<RedisClient> {
    let client = Client::open(redis_url)?;

    // Test connection
    let mut conn = client.get_multiplexed_async_connection().await?;
    let _: String = redis::cmd("PING").query_async(&mut conn).await?;

    tracing::info!("Redis connection established successfully");
    Ok(client)
}
