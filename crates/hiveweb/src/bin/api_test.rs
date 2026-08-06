//! Developer-only Assistant API client.
//!
//! Preferred usage keeps request bodies out of argv and shell history:
//!
//! ```text
//! trusted-request-body-producer | cargo run -p hiveweb --features dev-tools \
//!   --bin api-test -- GET /api/quota --body-stdin --host https://example.test
//! cargo run -p hiveweb --features dev-tools --bin api-test -- \
//!   POST /api/newsession --body-file /run/secrets/request.json
//! ```
//!
//! The legacy `<METHOD> <path> <json_body> [host]` form remains temporarily
//! compatible, but emits a deprecation warning.

use std::collections::HashMap;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{ArgGroup, Parser};

mod support;

use support::secret_input::read_private_file;

const MAX_BODY_INPUT_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Parser)]
#[command(
    about = "Send a signed request without printing request or response contents",
    group(
        ArgGroup::new("body_source")
            .required(true)
            .multiple(false)
            .args(["body_stdin", "body_file", "legacy_body"])
    )
)]
struct Cli {
    #[arg(value_name = "METHOD")]
    method: String,
    #[arg(value_name = "PATH")]
    path: String,
    /// DEPRECATED: request body in argv. Prefer --body-stdin or --body-file.
    #[arg(value_name = "JSON_BODY", hide = true)]
    legacy_body: Option<String>,
    /// Legacy positional host; valid only with the legacy positional body.
    #[arg(value_name = "LEGACY_HOST", requires = "legacy_body")]
    legacy_host: Option<String>,
    /// Read the request body from piped standard input.
    #[arg(long)]
    body_stdin: bool,
    /// Read the request body from a private file.
    #[arg(long, value_name = "PATH")]
    body_file: Option<PathBuf>,
    /// Explicit request origin for safe body-input modes.
    #[arg(
        long,
        value_name = "HTTP_OR_HTTPS_ORIGIN",
        conflicts_with = "legacy_host"
    )]
    host: Option<String>,
}

fn read_bounded_body(mut reader: impl Read, input_name: &str) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_BODY_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("failed to read API request {input_name}"))?;
    anyhow::ensure!(
        bytes.len() <= MAX_BODY_INPUT_BYTES as usize,
        "API request body is too large"
    );
    String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("API request body must be valid UTF-8"))
}

fn read_body_file(path: &Path) -> anyhow::Result<String> {
    let bytes = read_private_file(path, MAX_BODY_INPUT_BYTES, "API request body")?;
    String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("API request body must be valid UTF-8"))
}

fn request_body(cli: &Cli) -> anyhow::Result<String> {
    if cli.body_stdin {
        anyhow::ensure!(
            !std::io::stdin().is_terminal(),
            "--body-stdin requires piped input; use a mode-0600 --body-file for interactive use"
        );
        read_bounded_body(std::io::stdin().lock(), "body")
    } else if let Some(path) = cli.body_file.as_deref() {
        read_body_file(path)
    } else {
        let body = cli
            .legacy_body
            .as_ref()
            .context("request body source is required")?;
        anyhow::ensure!(
            body.len() <= MAX_BODY_INPUT_BYTES as usize,
            "API request body is too large"
        );
        eprintln!(
            "warning: positional request bodies are deprecated; use --body-stdin or --body-file"
        );
        Ok(body.clone())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let cli = Cli::parse();
    let body = request_body(&cli)?;
    let host = cli
        .host
        .as_deref()
        .or(cli.legacy_host.as_deref())
        .unwrap_or("127.0.0.1");

    let port = std::env::var("HIVEWEB_PORT").unwrap_or_else(|_| "3000".into());
    let secret = std::env::var("ASSISTANT_SECRET").unwrap_or_default();
    let base_url = resolve_base_url(host, &port)?;
    let method_upper = cli.method.to_uppercase();

    // Build sign body and request URL without logging any intermediate value.
    let url = match method_upper.as_str() {
        "GET" => {
            let params: HashMap<String, serde_json::Value> =
                serde_json::from_str(&body).context("invalid JSON body")?;
            let query = params
                .iter()
                .map(|(key, value)| {
                    let value = match value {
                        serde_json::Value::String(value) => value.clone(),
                        other => other.to_string(),
                    };
                    format!("{key}={value}")
                })
                .collect::<Vec<_>>()
                .join("&");
            let sign = make_sign(&secret, &cli.path, &query);
            format!("{base_url}{}?{query}&sign={sign}", cli.path)
        }
        "POST" => {
            serde_json::from_str::<serde_json::Value>(&body).context("invalid JSON body")?;
            let sign = make_sign(&secret, &cli.path, &body);
            format!("{base_url}{}?sign={sign}", cli.path)
        }
        _ => anyhow::bail!("unsupported method. Use GET or POST."),
    };

    let client = reqwest::Client::new();
    let request = match method_upper.as_str() {
        "GET" => client.get(&url),
        "POST" => client
            .post(&url)
            .header("Content-Type", "application/json; charset=UTF-8")
            .body(body),
        _ => unreachable!(),
    };
    let response = request
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("API test request failed"))?;
    let status = response.status();
    let response_body = response
        .bytes()
        .await
        .map_err(|_| anyhow::anyhow!("failed to read API test response"))?;
    println!(
        "{}",
        response_summary(&method_upper, status, &response_body)
    );
    Ok(())
}

fn response_summary(method: &str, status: reqwest::StatusCode, response_body: &[u8]) -> String {
    format!(
        "method: {method}\nstatus: {status}\nresponse_bytes: {}",
        response_body.len()
    )
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
        "unsupported host URL scheme. Use http or https."
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
    let sign_string = format!("{secret}{path}?body={body}");
    format!("{:x}", md5::compute(sign_string.as_bytes()))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use clap::Parser;

    use super::*;

    #[test]
    fn parser_supports_safe_body_sources_and_legacy_compatibility() {
        assert!(
            Cli::try_parse_from(["api-test", "POST", "/api/assistant", "--body-stdin"]).is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "api-test",
                "POST",
                "/api/assistant",
                "--body-file",
                "/run/secrets/request.json",
                "--host",
                "https://example.test",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "api-test",
                "POST",
                "/api/assistant",
                "{}",
                "https://example.test",
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["api-test", "POST", "/api/assistant"]).is_err());
        assert!(
            Cli::try_parse_from(["api-test", "POST", "/api/assistant", "{}", "--body-stdin",])
                .is_err()
        );
    }

    #[test]
    fn bounded_body_reader_rejects_invalid_utf8_and_oversized_input() {
        assert_eq!(
            read_bounded_body(Cursor::new(b"{\"ok\":true}"), "body").unwrap(),
            "{\"ok\":true}"
        );
        assert!(read_bounded_body(Cursor::new([0xff]), "body").is_err());
        assert!(
            read_bounded_body(
                Cursor::new(vec![b'x'; MAX_BODY_INPUT_BYTES as usize + 1]),
                "body",
            )
            .is_err()
        );
    }

    #[test]
    fn resolve_base_url_preserves_https_origin() {
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

    #[test]
    fn response_summary_never_contains_response_body() {
        let summary = response_summary(
            "POST",
            reqwest::StatusCode::BAD_REQUEST,
            b"SECRET_RESPONSE_BODY_SENTINEL",
        );
        assert_eq!(
            summary,
            "method: POST\nstatus: 400 Bad Request\nresponse_bytes: 29"
        );
        assert!(!summary.contains("SECRET_RESPONSE_BODY_SENTINEL"));
    }
}
