//! 测试 /api/quota 接口
//!
//! 用法:
//!   cargo run -p hiveweb --bin quota-test -- <host> <user_id>
//!
//! 环境变量:
//!   HIVEWEB_PORT   — 服务端口（默认 3000）
//!   ASSISTANT_SECRET — 签名密钥（未设置时不校验签名）

use anyhow::Context;
use std::env;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    tracing_subscriber::fmt::init();

    let user_id: i64 = env::args()
        .nth(1)
        .unwrap_or_default()
        .parse()
        .context("usage: quota-test <user_id> [host]")?;

    let host = env::args().nth(2).unwrap_or_else(|| "127.0.0.1".into());

    if user_id <= 0 {
        anyhow::bail!("user_id must be positive, got {user_id}");
    }

    let port = env::var("HIVEWEB_PORT").unwrap_or_else(|_| "3000".into());
    let secret = env::var("ASSISTANT_SECRET").unwrap_or_default();

    // 构造签名: MD5(secret + path + "?body=" + body)
    let path = "/api/quota";
    let body = format!("user_id={}", user_id);
    let sign_string = format!("{}{}?body={}", secret, path, body);
    let sign = format!("{:x}", md5::compute(sign_string.as_bytes()));

    let url = format!(
        "http://{}:{}/api/quota?sign={}&user_id={}",
        host, port, sign, user_id
    );

    tracing::info!(%url, "sending quota request");

    let resp = reqwest::get(&url)
        .await
        .context("failed to connect — is the server running?")?;

    let status = resp.status();
    let body_text = resp.text().await.context("failed to read response body")?;

    println!("status: {status}");
    println!("body:   {body_text}");

    Ok(())
}
