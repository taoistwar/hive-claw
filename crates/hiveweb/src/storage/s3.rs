use aws_sdk_s3::Client;
use aws_config::BehaviorVersion;

pub async fn create_client() -> anyhow::Result<Client> {
    let config = aws_config::defaults(BehaviorVersion::v2024_03_28())
        .load()
        .await;
    let client = Client::new(&config);

    tracing::info!("S3 client initialized successfully");
    Ok(client)
}
