//! 对外 API 接口测试工具
//!
//! 用法:
//!   cargo run -p hiveweb --features dev-tools --bin api-test -- <METHOD> <path> '<json_body>' [host] [--body-file <path> | --body-stdin]
//!
//! 示例:
//!   cargo run -p hiveweb --bin api-test -- GET /api/quota '{"user_id":448}' 172.16.208.113
//!   cargo run -p hiveweb --bin api-test -- GET /api/quota '{"user_id":448}' https://cca.haimacloud.com/
//!   cargo run -p hiveweb --bin api-test -- POST /api/newsession '{"user_id":448}'
//!   cargo run -p hiveweb --bin api-test -- POST /api/messages '{"user_id":448,"channel":"app","client_type":"android"}'
//!   cargo run -p hiveweb --bin api-test -- POST /api/assistant '{"user_id":448,"message":"你好","channel":"app","client_type":"android","client_version":"1.0.0"}' https://cca.haimacloud.com/
//!   cargo run -p hiveweb --bin api-test -- POST /api/recommended-games/top '{"user_id":"448","channel":"app","client_type":"android","client_version":"1.0.0"}'
//!   cargo run -p hiveweb --bin api-test -- POST /api/recommended-games/execute '{"user_id":"448","game_id":"1001","channel":"app","client_type":"android","client_version":"1.0.0"}'
//!   cargo run -p hiveweb --features dev-tools --bin api-test -- POST /api/assistant '{"user_id":448,"message":"你好"}' --body-stdin
//!   cargo run -p hiveweb --features dev-tools --bin api-test -- POST /api/assistant --body-file /tmp/body.json
//!
//! 环境变量:
//!   HIVEWEB_PORT      — 未指定协议的 host 所使用的服务端口（默认 3000）
//!   ASSISTANT_SECRET  — 签名密钥（未设置时不校验签名）

use anyhow::Context;
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::io::{self, Read};
use std::path::Path;

mod support;

use support::secret_input::read_private_file;

const BODY_SUMMARY_BYTES: usize = 1024;
const MAX_REQUEST_BODY_BYTES: u64 = 1024 * 1024;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let (method, path, body, host) = parse_args()?;

    let port = env::var("HIVEWEB_PORT").unwrap_or_else(|_| "3000".into());
    let secret = env::var("ASSISTANT_SECRET").unwrap_or_default();
    let base_url = resolve_base_url(&host, &port)?;

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
            let url = format!("{base_url}{path}?{qs}&sign={sign}");
            (sign_body, url)
        }
        "POST" => {
            serde_json::from_str::<serde_json::Value>(&body).context("invalid JSON body")?;
            let sign = make_sign(&secret, &path, &body);
            let url = format!("{base_url}{path}?sign={sign}");
            (body.clone(), url)
        }
        _ => anyhow::bail!("unsupported method: {method}. Use GET or POST."),
    };

    let client = reqwest::Client::new();
    let response = match method_upper.as_str() {
        "GET" => {
            let request = client.get(&url);
            request
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("API test request failed: {e}"))?
        }
        "POST" => {
            let request = client
                .post(&url)
                .header("Content-Type", "application/json; charset=UTF-8")
                .body(body);
            request
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("API test request failed: {e}"))?
        }
        _ => unreachable!(),
    };

    let status = response.status();
    let response_body = response.text().await.context("API test request failed")?;

    println!(
        "{}",
        response_summary(&method_upper, status, &response_body)
    );

    Ok(())
}

fn parse_args() -> anyhow::Result<(String, String, String, String)> {
    let args: Vec<String> = env::args().skip(1).collect();
    let usage = "usage: api-test <METHOD> <path> [json_body | --body-stdin | --body-file <path>] [host] (positional request bodies are deprecated)";
    let mut method: Option<String> = None;
    let mut path = None;
    let mut positional_body: Option<String> = None;
    let mut host = "127.0.0.1".to_string();
    let mut body: Option<String> = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--body-stdin" => {
                let mut stdin_body = String::new();
                io::stdin()
                    .read_to_string(&mut stdin_body)
                    .context("failed to read --body-stdin body")?;
                body = Some(stdin_body);
            }
            "--body-file" => {
                let path = args
                    .get(i + 1)
                    .context("missing argument for --body-file")?;
                let bytes =
                    read_private_file(Path::new(path), MAX_REQUEST_BODY_BYTES, "api-test body")?;
                let body_text = String::from_utf8(bytes)
                    .map_err(|_| anyhow::anyhow!("--body-file body must be valid utf-8"))?;
                body = Some(body_text);
                i += 1;
            }
            value if method.is_none() => method = Some(value.to_string()),
            value if path.is_none() => {
                path = Some(value.to_string());
            }
            value if positional_body.is_none() => {
                positional_body = Some(value.to_string());
            }
            value if host == "127.0.0.1" => host = value.to_string(),
            _ => anyhow::bail!("{}: {}", usage, args[i]),
        }
        i += 1;
    }

    let method = method.context(usage)?;
    let path = path.context(usage)?;
    let resolved_body = if let Some(body) = body {
        body
    } else if let Some(body) = positional_body {
        eprintln!("positional request bodies are deprecated");
        body
    } else {
        return Err(anyhow::anyhow!(usage));
    };

    Ok((method, path, resolved_body, host))
}

fn resolve_base_url(host: &str, port: &str) -> anyhow::Result<String> {
    let host = host.trim();
    anyhow::ensure!(!host.is_empty(), "host must not be empty");

    let candidate = if host.contains("://") {
        host.to_owned()
    } else {
        format!("http://{host}:{port}")
    };
    let url = reqwest::Url::parse(&candidate).context("invalid host URL")?;

    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https"),
        "unsupported host URL scheme: {}. Use http or https.",
        url.scheme()
    );
    anyhow::ensure!(url.host_str().is_some(), "host URL must include a hostname");
    anyhow::ensure!(
        url.query().is_none() && url.fragment().is_none(),
        "host URL must not include a query string or fragment"
    );

    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn make_sign(secret: &str, path: &str, body: &str) -> String {
    if secret.is_empty() {
        return String::new();
    }
    let sign_string = format!("{}{}?body={}", secret, path, body);
    format!("{:x}", md5::compute(sign_string.as_bytes()))
}

fn response_summary(method: &str, status: reqwest::StatusCode, response_body: &str) -> String {
    let is_json = serde_json::from_str::<Value>(response_body).is_ok();
    let body_bytes = response_body.len().min(BODY_SUMMARY_BYTES);
    let body_preview = &response_body[..body_bytes];
    let truncated = if body_bytes < response_body.len() {
        "...<truncated>"
    } else {
        ""
    };

    format!(
        "{method} {status} body={body_preview}{truncated} json={}",
        is_json,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_base_url_preserves_https_origin() {
        assert_eq!(
            resolve_base_url("https://cca.haimacloud.com/", "3000").unwrap(),
            "https://cca.haimacloud.com"
        );
    }

    #[test]
    fn test_get_quota() {
        assert_eq!(
            resolve_base_url("https://cca.haimacloud.com/", "3000").unwrap(),
            "https://cca.haimacloud.com"
        );
    }

    #[test]
    fn resolve_base_url_preserves_explicit_http_port() {
        assert_eq!(
            resolve_base_url("http://localhost:8080/", "3000").unwrap(),
            "http://localhost:8080"
        );
    }

    #[test]
    fn resolve_base_url_keeps_legacy_host_and_port_behavior() {
        assert_eq!(
            resolve_base_url("172.16.208.113", "3000").unwrap(),
            "http://172.16.208.113:3000"
        );
    }
}
