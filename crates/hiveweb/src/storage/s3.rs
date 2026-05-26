use aws_sdk_s3::Client;

pub async fn create_client() -> anyhow::Result<Client> {
    let config = aws_config::load_from_env().await;
    let client = Client::new(&config);

    tracing::info!("S3 client initialized successfully");
    Ok(client)
}
