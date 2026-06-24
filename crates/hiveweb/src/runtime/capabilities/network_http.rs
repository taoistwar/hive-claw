//! network.http capability handler (T095 / US4)
//!
//! 安全模型：
//! - hostname 必须命中 `NETWORK_HTTP_ALLOWLIST`（逗号分隔，env）；缺失 / 不命中 → 拒绝
//! - 拒绝私网地址（SSRF 防护）：127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, ::1, fc00::/7
//! - body 上下行各 ≤ 4 MB
//! - 不允许重定向（30x 不跟随；防 redirect-to-private）
//! - method ∈ {GET, POST, PUT, DELETE}

use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

const BODY_MAX_BYTES: usize = 4 * 1024 * 1024;

static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .expect("reqwest client build")
});

fn allowlist() -> Vec<String> {
    std::env::var("NETWORK_HTTP_ALLOWLIST")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_broadcast()
        }
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

#[derive(Debug, Deserialize)]
pub struct HttpArgs {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HttpReply {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
    pub body_truncated: bool,
}

pub async fn http_request(args: HttpArgs) -> Result<HttpReply, String> {
    let method = args.method.to_ascii_uppercase();
    if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "DELETE") {
        return Err(format!(
            "method {method} 不允许；仅支持 GET/POST/PUT/DELETE"
        ));
    }

    let url = reqwest::Url::parse(&args.url).map_err(|e| format!("url parse: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("仅允许 http/https scheme".into());
    }

    // 1. hostname allowlist
    let host = url
        .host_str()
        .ok_or_else(|| "url 缺少 host".to_string())?
        .to_ascii_lowercase();
    let list = allowlist();
    if list.is_empty() {
        return Err(
            "网络访问未配置；请设置 NETWORK_HTTP_ALLOWLIST 后重试（逗号分隔 hostname 列表）".into(),
        );
    }
    let allowed = list.iter().any(|h| {
        if let Some(suffix) = h.strip_prefix('.') {
            host.ends_with(suffix) || host == suffix
        } else {
            host == *h
        }
    });
    if !allowed {
        return Err(format!("hostname {host} 不在 allowlist 内"));
    }

    // 2. SSRF: resolve + check private
    if let Ok(addrs) = tokio::net::lookup_host(format!("{host}:0")).await {
        for sa in addrs {
            if is_private_ip(&sa.ip()) {
                return Err(format!("hostname {host} 解析到私网地址，拒绝（SSRF 防护）"));
            }
        }
    }

    // 3. build request
    let mut req = HTTP_CLIENT.request(
        method.parse().map_err(|e| format!("method parse: {e}"))?,
        url.clone(),
    );
    for (k, v) in &args.headers {
        req = req.header(k, v);
    }
    if let Some(body) = args.body.as_deref() {
        if body.len() > BODY_MAX_BYTES {
            return Err(format!("request body 大小 {} 超过 4 MB 上限", body.len()));
        }
        req = req.body(body.to_string());
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("HTTP 请求失败: {e}"))?;
    let status = resp.status().as_u16();
    let headers: HashMap<String, String> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let body_bytes = resp.bytes().await.map_err(|e| format!("read body: {e}"))?;
    let body_truncated = body_bytes.len() > BODY_MAX_BYTES;
    let body_slice = if body_truncated {
        &body_bytes[..BODY_MAX_BYTES]
    } else {
        &body_bytes[..]
    };
    let body = String::from_utf8_lossy(body_slice).to_string();
    Ok(HttpReply {
        status,
        headers,
        body,
        body_truncated,
    })
}
