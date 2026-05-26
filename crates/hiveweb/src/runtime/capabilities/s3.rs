//! s3.read / s3.write capability handlers (T097 / US4)
//!
//! 复用 `storage::s3` 的 client；bucket 限定为 env `S3_BUCKET`（与 Plugin 上传同桶；
//! Plugin 不可指定其它 bucket）。key 前缀必须以 `plugin-data/` 开头以隔离 Plugin
//! 写入区与系统区（WASM 文件在 `plugins/`）。

use aws_sdk_s3::Client as S3Client;
use serde::{Deserialize, Serialize};

const KEY_PREFIX: &str = "plugin-data/";

fn validate_key(key: &str) -> Result<(), String> {
    if !key.starts_with(KEY_PREFIX) {
        return Err(format!("key 必须以 '{KEY_PREFIX}' 开头"));
    }
    if key.contains("..") {
        return Err("key 包含 '..'，拒绝".into());
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct S3ReadArgs {
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct S3ReadReply {
    pub content: String,
    pub size_bytes: u64,
}

pub async fn s3_read(client: &S3Client, args: S3ReadArgs) -> Result<S3ReadReply, String> {
    validate_key(&args.key)?;
    let bytes = crate::storage::s3::get_wasm(client, &args.key)
        .await
        .map_err(|e| format!("s3 get_object: {e}"))?;
    let size_bytes = bytes.len() as u64;
    let content = String::from_utf8(bytes).map_err(|_| "content 非 UTF-8".to_string())?;
    Ok(S3ReadReply { content, size_bytes })
}

#[derive(Debug, Deserialize)]
pub struct S3WriteArgs {
    pub key: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct S3WriteReply {
    pub bytes_written: usize,
}

pub async fn s3_write(client: &S3Client, args: S3WriteArgs) -> Result<S3WriteReply, String> {
    validate_key(&args.key)?;
    let bytes = args.content.into_bytes();
    let n = bytes.len();
    crate::storage::s3::put_wasm(client, &args.key, bytes)
        .await
        .map_err(|e| format!("s3 put_object: {e}"))?;
    Ok(S3WriteReply { bytes_written: n })
}
