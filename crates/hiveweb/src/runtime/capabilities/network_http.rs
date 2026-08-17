//! network.http capability handler (T095 / US4)
//!
//! 安全模型：
//! - hostname 必须精确命中 `NETWORK_HTTP_ALLOWLIST`（逗号分隔，env）；不支持前导点或 wildcard 别名
//! - 拒绝私网、link-local、云 metadata 等非公开目标，并把预校验 DNS 结果固定到请求 Client
//! - body 上下行各 ≤ 4 MB
//! - 不允许重定向（30x 不跟随；防 redirect-to-private）
//! - method ∈ {GET, POST, PUT, DELETE, PATCH}
//! - 单次操作默认 5 秒、最大 30 秒

use futures::StreamExt;
use reqwest::{
    Client, Method, Url,
    header::{HOST, HeaderName, HeaderValue},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use super::CapabilityFailure;

const BODY_MAX_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_TIMEOUT_MS: u64 = 30_000;
const INVALID_ARGS_PREFIX: &str = "invalid capability arguments: ";

fn invalid_args(message: &str) -> String {
    format!("{INVALID_ARGS_PREFIX}{message}")
}

fn allowlist() -> Vec<String> {
    std::env::var("NETWORK_HTTP_ALLOWLIST")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn normalize_exact_hostname(host: &str) -> Option<String> {
    let normalized = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if normalized.is_empty()
        || normalized.starts_with('.')
        || normalized.contains('*')
        || normalized.contains('/')
    {
        None
    } else {
        Some(normalized)
    }
}

fn host_is_allowlisted(host: &str, configured: &[String]) -> bool {
    let Some(host) = normalize_exact_hostname(host) else {
        return false;
    };
    configured
        .iter()
        .filter_map(|entry| normalize_exact_hostname(entry))
        .any(|entry| entry == host)
}

fn is_metadata_host(host: &str) -> bool {
    matches!(
        host.trim_end_matches('.').to_ascii_lowercase().as_str(),
        "metadata.google.internal"
            | "metadata.azure.com"
            | "instance-data.ec2.internal"
            | "metadata.oraclecloud.com"
    )
}

fn is_blocked_ipv4(ip: &Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_documentation()
        || octets[0] == 0
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 198 && (18..=19).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 192 && octets[1] == 88 && octets[2] == 99)
        || octets[0] >= 240
}

fn is_blocked_ipv6(ip: &Ipv6Addr) -> bool {
    let segments = ip.segments();
    let first = segments[0];
    let global_unicast = first & 0xe000 == 0x2000;
    let ipv4_compatible = segments[..6].iter().all(|segment| *segment == 0);
    let discard_only = first == 0x0100 && segments[1..4].iter().all(|segment| *segment == 0);
    let special_2001 = first == 0x2001 && segments[1] < 0x0200;
    let documentation = first == 0x2001 && segments[1] == 0x0db8;
    let six_to_four = first == 0x2002;
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || !global_unicast
        || ipv4_compatible
        || discard_only
        || special_2001
        || documentation
        || six_to_four
        || first & 0xfe00 == 0xfc00
        || first & 0xffc0 == 0xfe80
        || first & 0xffc0 == 0xfec0
        || ip.to_ipv4_mapped().as_ref().is_some_and(is_blocked_ipv4)
}

fn is_blocked_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_blocked_ipv4(ip),
        IpAddr::V6(ip) => is_blocked_ipv6(ip),
    }
}

/// Scheme constraint for shared outbound HTTP users.
///
/// Hook Webhooks deliberately use `HttpsOnly`; `network.http` retains its
/// documented HTTP-or-HTTPS contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutboundScheme {
    HttpOrHttps,
    HttpsOnly,
}

/// Typed target-resolution result used by all outbound HTTP callers.
///
/// Policy failures are deterministic and must never be retried. Resolver
/// unavailability is operational/transient and may be retried by Hook
/// Webhooks. Payloads are static safe messages: raw resolver details and host
/// input never cross this boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutboundTargetError {
    Policy(&'static str),
    ResolveUnavailable(&'static str),
}

impl OutboundTargetError {
    fn into_safe_message(self) -> String {
        match self {
            Self::Policy(message) => invalid_args(message),
            Self::ResolveUnavailable(message) => message.to_string(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ParsedOutboundTarget {
    pub(crate) url: Url,
    pub(crate) host: String,
    port: u16,
}

#[derive(Debug)]
pub(crate) struct ResolvedOutboundTarget {
    pub(crate) url: Url,
    pub(crate) host: String,
    addresses: Vec<SocketAddr>,
}

/// Parse an outbound URL and apply the policy that must run before DNS.
///
/// Keeping this in the `network.http` module gives Hook Webhooks the same URL,
/// metadata-host, literal-IP and credential checks as the capability instead
/// of maintaining a second string blacklist.
pub(crate) fn parse_outbound_target(
    raw_url: &str,
    scheme_policy: OutboundScheme,
) -> Result<ParsedOutboundTarget, OutboundTargetError> {
    let url = Url::parse(raw_url).map_err(|_| OutboundTargetError::Policy("URL is invalid"))?;
    let allowed_scheme = match scheme_policy {
        OutboundScheme::HttpOrHttps => matches!(url.scheme(), "http" | "https"),
        OutboundScheme::HttpsOnly => url.scheme() == "https",
    };
    if !allowed_scheme {
        return Err(OutboundTargetError::Policy(match scheme_policy {
            OutboundScheme::HttpOrHttps => "only http and https schemes are supported",
            OutboundScheme::HttpsOnly => "only https scheme is supported",
        }));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(OutboundTargetError::Policy(
            "URL must not contain credentials",
        ));
    }

    let host = url
        .host_str()
        .ok_or(OutboundTargetError::Policy("URL has no hostname"))?
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    if is_metadata_host(&host) {
        return Err(OutboundTargetError::Policy(
            "target hostname is blocked by the SSRF policy",
        ));
    }
    if host
        .parse::<IpAddr>()
        .is_ok_and(|address| is_blocked_ip(&address))
    {
        return Err(OutboundTargetError::Policy(
            "target address is blocked by the SSRF policy",
        ));
    }
    let port = url
        .port_or_known_default()
        .ok_or(OutboundTargetError::Policy("URL has no valid port"))?;

    Ok(ParsedOutboundTarget { url, host, port })
}

/// Reject the entire DNS answer set when any answer is not publicly routable.
///
/// This "all answers" rule prevents a hostname with one public and one
/// loopback/private answer from selecting the unsafe address at connect time.
pub(crate) fn validate_public_addresses(
    host: &str,
    addresses: &[SocketAddr],
) -> Result<(), OutboundTargetError> {
    if is_metadata_host(host) {
        return Err(OutboundTargetError::Policy(
            "target hostname is blocked by the SSRF policy",
        ));
    }
    if addresses.is_empty() {
        return Err(OutboundTargetError::ResolveUnavailable(
            "hostname DNS 未返回地址",
        ));
    }
    if addresses.iter().any(|addr| is_blocked_ip(&addr.ip())) {
        return Err(OutboundTargetError::Policy(
            "target hostname resolves to an address blocked by the SSRF policy",
        ));
    }
    Ok(())
}

pub(crate) async fn resolve_parsed_outbound_target(
    parsed: ParsedOutboundTarget,
) -> Result<ResolvedOutboundTarget, OutboundTargetError> {
    let resolved = tokio::net::lookup_host((parsed.host.as_str(), parsed.port))
        .await
        .map_err(|_| OutboundTargetError::ResolveUnavailable("hostname DNS 解析失败"))?;
    let mut addresses: Vec<SocketAddr> = resolved.collect();
    addresses.sort_unstable();
    addresses.dedup();
    validate_public_addresses(&parsed.host, &addresses)?;

    Ok(ResolvedOutboundTarget {
        url: parsed.url,
        host: parsed.host,
        addresses,
    })
}

pub(crate) async fn resolve_outbound_target(
    raw_url: &str,
    scheme_policy: OutboundScheme,
) -> Result<ResolvedOutboundTarget, OutboundTargetError> {
    let parsed = parse_outbound_target(raw_url, scheme_policy)?;
    resolve_parsed_outbound_target(parsed).await
}

fn effective_timeout_ms(timeout_ms: Option<u64>) -> Result<u64, String> {
    match timeout_ms {
        Some(0) => Err(invalid_args("timeout_ms must be greater than zero")),
        Some(timeout_ms) => Ok(timeout_ms.min(MAX_TIMEOUT_MS)),
        None => Ok(DEFAULT_TIMEOUT_MS),
    }
}

fn validate_method_and_body(method: &str, has_body: bool) -> Result<Method, String> {
    let method = match method.trim().to_ascii_uppercase().as_str() {
        "GET" => Method::GET,
        "POST" => Method::POST,
        "PUT" => Method::PUT,
        "DELETE" => Method::DELETE,
        "PATCH" => Method::PATCH,
        _ => return Err(invalid_args("unsupported HTTP method")),
    };
    if has_body && !matches!(method, Method::POST | Method::PUT | Method::PATCH) {
        return Err(invalid_args(
            "body is only supported for POST, PUT, or PATCH",
        ));
    }
    Ok(method)
}

fn pinned_client_builder(host: &str, addresses: &[SocketAddr]) -> reqwest::ClientBuilder {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve_to_addrs(host, addresses)
}

fn pinned_client(host: &str, addresses: &[SocketAddr], timeout_ms: u64) -> Result<Client, String> {
    pinned_client_builder(host, addresses)
        .timeout(Duration::from_millis(timeout_ms))
        .build()
        .map_err(|_| "HTTP client 初始化失败".to_string())
}

pub(crate) fn pinned_outbound_client(
    target: &ResolvedOutboundTarget,
    timeout_ms: u64,
) -> Result<Client, String> {
    pinned_client(&target.host, &target.addresses, timeout_ms)
}

/// Build the same DNS-pinned/no-proxy/no-redirect client without installing a
/// second request timer. Hook Webhooks wrap DNS + construction + send in one
/// Tokio timeout, while `network.http` retains its existing client timeout.
#[allow(dead_code)]
pub(crate) fn pinned_outbound_client_for_attempt(
    target: &ResolvedOutboundTarget,
) -> Result<Client, String> {
    pinned_client_builder(&target.host, &target.addresses)
        .build()
        .map_err(|_| "HTTP client 初始化失败".to_string())
}

pub(crate) fn parse_outbound_header(
    name: &str,
    value: &str,
) -> Result<(HeaderName, HeaderValue), String> {
    let name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| invalid_args("HTTP header name is invalid"))?;
    if name == HOST {
        return Err(invalid_args("Host header cannot be overridden"));
    }
    let value =
        HeaderValue::from_str(value).map_err(|_| invalid_args("HTTP header value is invalid"))?;
    Ok((name, value))
}

fn append_response_chunk(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), String> {
    if chunk.len() > BODY_MAX_BYTES.saturating_sub(body.len()) {
        return Err("HTTP 响应超过 4 MB 上限".into());
    }
    body.extend_from_slice(chunk);
    Ok(())
}

async fn run_with_operation_timeout<T, F>(timeout_ms: u64, future: F) -> Result<T, String>
where
    F: Future<Output = Result<T, String>>,
{
    tokio::time::timeout(Duration::from_millis(timeout_ms), future)
        .await
        .map_err(|_| "capability timeout".to_string())?
}

#[derive(Debug, Deserialize)]
pub struct HttpArgs {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct HttpReply {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

fn classify_http_failure(error: String) -> CapabilityFailure {
    if error.starts_with(INVALID_ARGS_PREFIX) {
        CapabilityFailure::invalid_arguments()
    } else if error == "capability timeout" {
        CapabilityFailure::timeout()
    } else {
        CapabilityFailure::failed(error)
    }
}

pub async fn http_request(args: HttpArgs) -> Result<HttpReply, CapabilityFailure> {
    let timeout_ms = effective_timeout_ms(args.timeout_ms).map_err(classify_http_failure)?;
    run_with_operation_timeout(timeout_ms, http_request_inner(args, timeout_ms))
        .await
        .map_err(classify_http_failure)
}

async fn http_request_inner(args: HttpArgs, timeout_ms: u64) -> Result<HttpReply, String> {
    let method = validate_method_and_body(&args.method, args.body.is_some())?;
    let parsed = parse_outbound_target(&args.url, OutboundScheme::HttpOrHttps)
        .map_err(OutboundTargetError::into_safe_message)?;

    // 1. hostname allowlist
    let list = allowlist();
    if list.is_empty() {
        return Err(
            "网络访问未配置；请设置 NETWORK_HTTP_ALLOWLIST 后重试（逗号分隔 hostname 列表）".into(),
        );
    }
    if !host_is_allowlisted(&parsed.host, &list) {
        return Err(invalid_args(
            "hostname is not present in the exact allowlist",
        ));
    }

    // 2. SSRF: resolve, validate every answer, then pin those answers into the
    // reqwest client so the actual connection cannot perform a second DNS lookup.
    let target = resolve_parsed_outbound_target(parsed)
        .await
        .map_err(OutboundTargetError::into_safe_message)?;
    let client = pinned_outbound_client(&target, timeout_ms)?;

    // 3. build request
    let mut req = client.request(method, target.url);
    for (k, v) in &args.headers {
        let (name, value) = parse_outbound_header(k, v)?;
        req = req.header(name, value);
    }
    if let Some(body) = args.body.as_deref() {
        if body.len() > BODY_MAX_BYTES {
            return Err(invalid_args("request body exceeds the 4 MiB limit"));
        }
        req = req.body(body.to_string());
    }

    let resp = req.send().await.map_err(|error| {
        if error.is_timeout() {
            "capability timeout".to_string()
        } else {
            "HTTP 请求失败".to_string()
        }
    })?;
    let status = resp.status().as_u16();
    let headers: HashMap<String, String> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let mut stream = resp.bytes_stream();
    let mut body_bytes = Vec::with_capacity(BODY_MAX_BYTES.min(64 * 1024));
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            if error.is_timeout() {
                "capability timeout".to_string()
            } else {
                "HTTP 响应读取失败".to_string()
            }
        })?;
        append_response_chunk(&mut body_bytes, &chunk)?;
    }
    let body = String::from_utf8_lossy(&body_bytes).to_string();
    Ok(HttpReply {
        status,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        BODY_MAX_BYTES, DEFAULT_TIMEOUT_MS, MAX_TIMEOUT_MS, OutboundScheme, OutboundTargetError,
        append_response_chunk, effective_timeout_ms, host_is_allowlisted, is_blocked_ip,
        is_metadata_host, parse_outbound_header, parse_outbound_target, pinned_client,
        resolve_parsed_outbound_target, run_with_operation_timeout, validate_method_and_body,
        validate_public_addresses,
    };
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn allowlist_is_exact_and_rejects_leading_dot_aliases() {
        let configured = vec![
            "EXAMPLE.COM".to_string(),
            ".legacy.example".to_string(),
            "*.wildcard.example".to_string(),
        ];

        assert!(host_is_allowlisted("example.com", &configured));
        assert!(!host_is_allowlisted("api.example.com", &configured));
        assert!(!host_is_allowlisted("legacy.example", &configured));
        assert!(!host_is_allowlisted("sub.legacy.example", &configured));
        assert!(!host_is_allowlisted("api.wildcard.example", &configured));
    }

    #[test]
    fn timeout_defaults_caps_and_rejects_zero() {
        assert_eq!(effective_timeout_ms(None).unwrap(), DEFAULT_TIMEOUT_MS);
        assert_eq!(effective_timeout_ms(Some(3_000)).unwrap(), 3_000);
        assert_eq!(
            effective_timeout_ms(Some(MAX_TIMEOUT_MS + 1)).unwrap(),
            MAX_TIMEOUT_MS
        );
        assert!(effective_timeout_ms(Some(0)).is_err());
    }

    #[tokio::test]
    async fn operation_timeout_covers_work_before_the_http_client_exists() {
        let result = run_with_operation_timeout(1, async {
            tokio::time::sleep(Duration::from_millis(25)).await;
            Ok::<_, String>(())
        })
        .await;

        assert_eq!(result.unwrap_err(), "capability timeout");
    }

    #[test]
    fn patch_accepts_a_body_but_get_and_delete_do_not() {
        assert_eq!(
            validate_method_and_body("patch", true).unwrap().as_str(),
            "PATCH"
        );
        assert_eq!(
            validate_method_and_body("POST", true).unwrap().as_str(),
            "POST"
        );
        assert!(validate_method_and_body("GET", true).is_err());
        assert!(validate_method_and_body("DELETE", true).is_err());
        assert!(validate_method_and_body("CONNECT", false).is_err());
    }

    #[test]
    fn metadata_names_and_non_public_addresses_are_blocked() {
        for host in [
            "metadata.google.internal",
            "metadata.azure.com",
            "instance-data.ec2.internal",
        ] {
            assert!(is_metadata_host(host), "{host} must be blocked");
        }
        assert!(!is_metadata_host("api.example.com"));

        for ip in [
            IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
            IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)),
            IpAddr::V4(Ipv4Addr::new(240, 0, 0, 1)),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6("100::1".parse().unwrap()),
            IpAddr::V6("2001:db8::1".parse().unwrap()),
            IpAddr::V6("2002:0a00:1::1".parse().unwrap()),
            IpAddr::V6("fc00::1".parse().unwrap()),
            IpAddr::V6("fe80::1".parse().unwrap()),
        ] {
            assert!(is_blocked_ip(&ip), "{ip} must be blocked");
        }
        assert!(!is_blocked_ip(&IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));
        assert!(!is_blocked_ip(&IpAddr::V6(
            "2606:2800:220:1:248:1893:25c8:1946".parse().unwrap()
        )));
    }

    #[test]
    fn shared_outbound_policy_requires_https_for_webhooks_and_rejects_credentials() {
        assert!(
            parse_outbound_target("http://example.com/hook", OutboundScheme::HttpsOnly).is_err()
        );
        assert!(
            parse_outbound_target(
                "https://user:password@example.com/hook",
                OutboundScheme::HttpsOnly
            )
            .is_err()
        );
        assert!(
            parse_outbound_target("https://example.com/hook", OutboundScheme::HttpsOnly).is_ok()
        );
    }

    #[test]
    fn shared_outbound_policy_rejects_any_private_dns_answer_including_ipv6() {
        let public = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)), 443);
        let private_v6 = SocketAddr::new(IpAddr::V6("fd00::1".parse().unwrap()), 443);

        assert!(
            validate_public_addresses("example.com", &[public, private_v6]).is_err(),
            "one private DNS answer must reject the whole target"
        );
        assert!(parse_outbound_target("https://[::1]/hook", OutboundScheme::HttpsOnly).is_err());
    }

    #[test]
    fn outbound_target_errors_distinguish_policy_from_resolver_unavailability() {
        let policy = parse_outbound_target("http://example.com/hook", OutboundScheme::HttpsOnly)
            .unwrap_err();
        assert!(matches!(policy, OutboundTargetError::Policy(_)));

        let unavailable = validate_public_addresses("example.com", &[]).unwrap_err();
        assert!(matches!(
            unavailable,
            OutboundTargetError::ResolveUnavailable(_)
        ));

        let metadata = validate_public_addresses("metadata.google.internal", &[]).unwrap_err();
        assert!(matches!(metadata, OutboundTargetError::Policy(_)));

        let private_answer = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 443);
        let policy = validate_public_addresses("example.com", &[private_answer]).unwrap_err();
        assert!(matches!(policy, OutboundTargetError::Policy(_)));
    }

    #[tokio::test]
    async fn nxdomain_is_typed_as_resolver_unavailability() {
        let parsed = parse_outbound_target(
            "https://hiveweb-resolver-contract.invalid/hook",
            OutboundScheme::HttpsOnly,
        )
        .unwrap();
        let error = resolve_parsed_outbound_target(parsed).await.unwrap_err();
        assert!(matches!(error, OutboundTargetError::ResolveUnavailable(_)));
    }

    #[test]
    fn shared_outbound_headers_reject_host_override_and_injection() {
        assert!(parse_outbound_header("x-hook-event", "ready").is_ok());
        assert!(parse_outbound_header("Host", "internal.example").is_err());
        assert!(parse_outbound_header("x-hook-event", "ready\r\nx-forged: yes").is_err());
    }

    #[tokio::test]
    async fn pinned_transport_uses_validated_address_and_does_not_follow_redirects() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let server_hits = Arc::clone(&hits);
        let server = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let hit = server_hits.fetch_add(1, Ordering::SeqCst) + 1;
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).await;
                let response = if hit == 1 {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{}/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        address.port()
                    )
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                        .to_string()
                };
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let client = pinned_client("safe.test", &[address], 1_000).unwrap();
        let response = client
            .get(format!("http://safe.test:{}/start", address.port()))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[test]
    fn response_body_limit_is_an_error_instead_of_a_truncated_success() {
        let mut body = vec![b'a'; BODY_MAX_BYTES];
        assert!(append_response_chunk(&mut body, &[]).is_ok());
        assert!(append_response_chunk(&mut body, b"x").is_err());
        assert_eq!(body.len(), BODY_MAX_BYTES);
    }
}
