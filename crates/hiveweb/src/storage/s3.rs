use aws_config::BehaviorVersion;
use aws_sdk_s3::{Client, config::Region};

pub async fn create_client() -> anyhow::Result<Client> {
    let endpoint_url = std::env::var("AWS_ENDPOINT_URL")
        .map_err(|_| anyhow::anyhow!("AWS_ENDPOINT_URL env var missing"))?;
    let access_key = std::env::var("AWS_ACCESS_KEY_ID")
        .map_err(|_| anyhow::anyhow!("AWS_ACCESS_KEY_ID env var missing"))?;
    let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY")
        .map_err(|_| anyhow::anyhow!("AWS_SECRET_ACCESS_KEY env var missing"))?;
    let region = std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());

    let config = aws_config::defaults(BehaviorVersion::v2025_08_07())
        .endpoint_url(endpoint_url)
        .credentials_provider(aws_sdk_s3::config::Credentials::new(
            access_key, secret_key, None, None, "env",
        ))
        .load()
        .await;

    let client = Client::from_conf(
        aws_sdk_s3::config::Builder::from(&config)
            .region(Region::new(region))
            .build(),
    );

    tracing::info!("S3 client initialized successfully");
    Ok(client)
}

fn bucket() -> anyhow::Result<String> {
    std::env::var("S3_BUCKET").map_err(|_| anyhow::anyhow!("S3_BUCKET env var missing"))
}

/// 004 Agent Runtime — Plugin WASM 上传 / 下载 / 删除（FR-005 / T072）
pub async fn put_wasm(client: &Client, key: &str, bytes: Vec<u8>) -> anyhow::Result<()> {
    client
        .put_object()
        .bucket(bucket()?)
        .key(key)
        .body(bytes.into())
        .content_type("application/wasm")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("S3 put_object failed: {e}"))?;
    Ok(())
}

pub async fn get_wasm(client: &Client, key: &str) -> anyhow::Result<Vec<u8>> {
    let resp = client
        .get_object()
        .bucket(bucket()?)
        .key(key)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("S3 get_object failed: {e}"))?;
    let bytes = resp
        .body
        .collect()
        .await
        .map_err(|e| anyhow::anyhow!("S3 stream read failed: {e}"))?
        .into_bytes()
        .to_vec();
    Ok(bytes)
}

pub async fn delete_wasm(client: &Client, key: &str) -> anyhow::Result<()> {
    client
        .delete_object()
        .bucket(bucket()?)
        .key(key)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("S3 delete_object failed: {e}"))?;
    Ok(())
}
