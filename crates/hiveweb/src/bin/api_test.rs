//! 对外 API 接口测试工具
//!
//! 用法:
//!   cargo run -p hiveweb --bin api-test -- <METHOD> <path> '<json_body>' [host]
//!
//! 示例:
//!   cargo run -p hiveweb --bin api-test -- GET /api/quota '{"user_id":123}' 172.16.208.113
//!   cargo run -p hiveweb --bin api-test -- POST /api/newsession '{"user_id":123}'
//!   cargo run -p hiveweb --bin api-test -- POST /api/messages '{"user_id":123,"channel":"app","client_type":"android"}'
//!   cargo run -p hiveweb --bin api-test -- POST /api/assistant '{"user_id":123,"message":"你好","channel":"app","client_type":"android","client_version":"1.0.0"}'
//!   cargo run -p hiveweb --bin api-test -- POST /api/recommended-games/top '{"user_id":"123","channel":"app","client_type":"android","client_version":"1.0.0"}'
//!   cargo run -p hiveweb --bin api-test -- POST /api/recommended-games/execute '{"user_id":"123","game_id":"1001","channel":"app","client_type":"android","client_version":"1.0.0"}'
//!
//! 环境变量:
//!   HIVEWEB_PORT      — 服务端口（默认 3000）
//!   ASSISTANT_SECRET  — 签名密钥（未设置时不校验签名）

use anyhow::Context;
use std::collections::HashMap;
use std::env;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    tracing_subscriber::fmt::init();

    let method = env::args()
        .nth(1)
        .context("usage: api-test <METHOD> <path> <json_body> [host]")?;
    let path = env::args()
        .nth(2)
        .context("usage: api-test <METHOD> <path> <json_body> [host]")?;
    let body = env::args()
        .nth(3)
        .context("usage: api-test <METHOD> <path> <json_body> [host]")?;
    let host = env::args().nth(4).unwrap_or_else(|| "127.0.0.1".into());

    let port = env::var("HIVEWEB_PORT").unwrap_or_else(|_| "3000".into());
    let secret = env::var("ASSISTANT_SECRET").unwrap_or_default();

    let method_upper = method.to_uppercase();

    // Build sign body and request URL
    let (sign_body, url) = match method_upper.as_str() {
        "GET" => {
            // GET: parse JSON into flat key=value pairs, use as query string
            let params: HashMap<String, serde_json::Value> =
                serde_json::from_str(&body).context("invalid JSON body")?;
            let qs: Vec<String> = params
                .iter()
                .map(|(k, v)| {
                    let val = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    format!("{}={}", k, val)
                })
                .collect();
            let qs = qs.join("&");

            // Sign body for GET is the query string (without sign param)
            let sign_body = qs.clone();

            let sign = make_sign(&secret, &path, &sign_body);
            let url = format!("http://{}:{}{}?{}&sign={}", host, port, path, qs, sign);
            (sign_body, url)
        }
        "POST" => {
            serde_json::from_str::<serde_json::Value>(&body).context("invalid JSON body")?;
            let sign = make_sign(&secret, &path, &body);
            let url = format!("http://{}:{}{}?sign={}", host, port, path, sign);
            (body.clone(), url)
        }
        _ => anyhow::bail!("unsupported method: {method}. Use GET or POST."),
    };

    let client = reqwest::Client::new();
    tracing::info!(method = %method_upper, %path, %host, %sign_body, "sending request");

    let resp = match method_upper.as_str() {
        "GET" => client.get(&url).send().await?,
        "POST" => {
            client
                .post(&url)
                .header("Content-Type", "application/json; charset=UTF-8")
                .body(body)
                .send()
                .await?
        }
        _ => unreachable!(),
    };

    let status = resp.status();
    let body_text = resp.text().await.context("failed to read response body")?;

    // Pretty-print JSON responses
    let display_body = if let Ok(val) = serde_json::from_str::<serde_json::Value>(&body_text) {
        serde_json::to_string_pretty(&val).unwrap_or(body_text)
    } else {
        body_text
    };

    println!("status: {status}");
    println!();
    println!("{display_body}");

    Ok(())
}

fn make_sign(secret: &str, path: &str, body: &str) -> String {
    if secret.is_empty() {
        return String::new();
    }
    let sign_string = format!("{}{}?body={}", secret, path, body);
    format!("{:x}", md5::compute(sign_string.as_bytes()))
}
