use aws_config::BehaviorVersion;
use aws_sdk_s3::{Client, config::Region};

const FORCE_PATH_STYLE_ENV: &str = "AWS_S3_FORCE_PATH_STYLE";

pub async fn create_client() -> anyhow::Result<Client> {
    let endpoint_url = std::env::var("AWS_ENDPOINT_URL")
        .map_err(|_| anyhow::anyhow!("AWS_ENDPOINT_URL env var missing"))?;
    let access_key = std::env::var("AWS_ACCESS_KEY_ID")
        .map_err(|_| anyhow::anyhow!("AWS_ACCESS_KEY_ID env var missing"))?;
    let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY")
        .map_err(|_| anyhow::anyhow!("AWS_SECRET_ACCESS_KEY env var missing"))?;
    let region = std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let force_path_style = force_path_style_from_env()?;

    let config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .endpoint_url(endpoint_url)
        .credentials_provider(aws_sdk_s3::config::Credentials::new(
            access_key, secret_key, None, None, "env",
        ))
        .load()
        .await;

    let client = build_client(&config, region, force_path_style);

    tracing::info!(force_path_style, "S3 client initialized successfully");
    Ok(client)
}

fn build_client(config: &aws_config::SdkConfig, region: String, force_path_style: bool) -> Client {
    Client::from_conf(
        aws_sdk_s3::config::Builder::from(config)
            .region(Region::new(region))
            .force_path_style(force_path_style)
            .build(),
    )
}

fn force_path_style_from_env() -> anyhow::Result<bool> {
    match std::env::var(FORCE_PATH_STYLE_ENV) {
        Ok(value) => parse_force_path_style(Some(&value)),
        Err(std::env::VarError::NotPresent) => parse_force_path_style(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(anyhow::anyhow!(
            "{FORCE_PATH_STYLE_ENV} must be valid UTF-8 and one of \
             true, false, 1, 0, yes, no, on, or off"
        )),
    }
}

fn parse_force_path_style(value: Option<&str>) -> anyhow::Result<bool> {
    let Some(value) = value else {
        return Ok(true);
    };

    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(anyhow::anyhow!(
            "{FORCE_PATH_STYLE_ENV} must be one of \
             true, false, 1, 0, yes, no, on, or off"
        )),
    }
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aws_config::BehaviorVersion;
    use aws_sdk_s3::{
        config::{Credentials, Region},
        presigning::PresigningConfig,
    };

    use super::{FORCE_PATH_STYLE_ENV, build_client, parse_force_path_style};

    #[test]
    fn path_style_parser_accepts_documented_boolean_values() {
        for value in ["true", "TRUE", "1", "yes", "YeS", "on", " on "] {
            assert!(
                parse_force_path_style(Some(value)).expect("documented true value should parse"),
                "{value:?} should enable path-style addressing"
            );
        }

        for value in ["false", "FALSE", "0", "no", "No", "off", " off "] {
            assert!(
                !parse_force_path_style(Some(value)).expect("documented false value should parse"),
                "{value:?} should disable path-style addressing"
            );
        }
    }

    #[test]
    fn path_style_parser_defaults_to_enabled_when_unset() {
        assert!(
            parse_force_path_style(None).expect("missing value should use the documented default")
        );
    }

    #[test]
    fn path_style_parser_rejects_invalid_values_without_echoing_them() {
        let invalid_value = "secret-like-invalid-value";
        let error = parse_force_path_style(Some(invalid_value))
            .expect_err("an undocumented value must not silently change S3 addressing");
        let message = error.to_string();

        assert!(message.contains(FORCE_PATH_STYLE_ENV));
        assert!(message.contains("true"));
        assert!(!message.contains(invalid_value));
    }

    #[test]
    fn path_style_parser_rejects_explicit_empty_value() {
        let error = parse_force_path_style(Some(" \t "))
            .expect_err("an explicit empty value should be treated as invalid configuration");

        assert!(error.to_string().contains(FORCE_PATH_STYLE_ENV));
    }

    #[tokio::test]
    async fn enabled_path_style_keeps_bucket_in_custom_endpoint_path() {
        let shared_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
            .endpoint_url("http://127.0.0.1:9000")
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(
                "test-access-key",
                "test-secret-key",
                None,
                None,
                "test",
            ))
            .load()
            .await;
        let client = build_client(&shared_config, "us-east-1".to_owned(), true);

        let request = client
            .get_object()
            .bucket("hiveweb-test")
            .key("plugins/example.wasm")
            .presigned(
                PresigningConfig::expires_in(Duration::from_secs(60))
                    .expect("test presigning duration should be valid"),
            )
            .await
            .expect("path-style request should presign without network access");
        let unsigned_uri = request
            .uri()
            .split_once('?')
            .map_or(request.uri(), |(uri, _)| uri);

        assert_eq!(
            unsigned_uri,
            "http://127.0.0.1:9000/hiveweb-test/plugins/example.wasm"
        );
        assert!(!request.uri().contains("hiveweb-test.127.0.0.1"));
    }
}
