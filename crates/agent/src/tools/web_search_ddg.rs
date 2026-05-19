//! DuckDuckGo [`WebSearchBackend`] implementation.
//!
//! Port of `nanobot.agent.tools.web._search_duckduckgo`. Python uses
//! the third-party `ddgs` library; under the hood it POSTs to
//! `https://html.duckduckgo.com/html/` and scrapes the response. We do
//! the same directly with `reqwest` + `regex`, so no API key is needed
//! and we avoid a heavy HTML parser dependency.

use std::time::Duration;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;

use super::web::{WebSearchBackend, WebSearchItem};

const DDG_HTML_ENDPOINT: &str = "https://html.duckduckgo.com/html/";
const DDG_LITE_ENDPOINT: &str = "https://lite.duckduckgo.com/lite/";
const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_7_2) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

static RESULT_A_RE: Lazy<Regex> = Lazy::new(|| {
    // Matches `<a ... class="result__a" ... href="..." ...>title html</a>`.
    // class="result__a" may appear before or after href.
    Regex::new(r#"(?is)<a\b[^>]*\bclass="[^"]*\bresult__a\b[^"]*"[^>]*\bhref="([^"]+)"[^>]*>(.*?)</a>"#)
        .unwrap()
});

static SNIPPET_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?is)<a\b[^>]*\bclass="[^"]*\bresult__snippet\b[^"]*"[^>]*>(.*?)</a>"#).unwrap()
});

static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());
static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
static UDDG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[?&]uddg=([^&]+)").unwrap());

/// DuckDuckGo HTML-scraping backend.
pub struct DuckDuckGoBackend {
    client: reqwest::Client,
    timeout: Duration,
}

impl DuckDuckGoBackend {
    pub fn new() -> Self {
        Self::with_timeout(Duration::from_secs(10))
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::limited(5))
            .timeout(timeout)
            .build()
            .expect("reqwest client");
        Self { client, timeout }
    }

    pub fn with_client(client: reqwest::Client, timeout: Duration) -> Self {
        Self { client, timeout }
    }

    async fn fetch_html(&self, query: &str) -> Result<String, String> {
        let do_post = |endpoint: &'static str| {
            self.client
                .post(endpoint)
                .form(&[("q", query), ("b", ""), ("kl", ""), ("df", "")])
                .header("Accept", "text/html,application/xhtml+xml")
                .send()
        };
        // Try the full HTML endpoint first; fall back to the lite endpoint
        // (same markup, smaller response, sometimes less rate-limited).
        match tokio::time::timeout(self.timeout + Duration::from_secs(2), do_post(DDG_HTML_ENDPOINT))
            .await
        {
            Ok(Ok(resp)) if resp.status().is_success() => resp.text().await.map_err(err),
            Ok(Ok(resp)) => {
                log::warn!("DDG html endpoint returned {}, trying lite", resp.status());
                let resp = do_post(DDG_LITE_ENDPOINT).await.map_err(err)?;
                resp.text().await.map_err(err)
            }
            Ok(Err(e)) => {
                log::warn!("DDG html endpoint error: {e}, trying lite");
                let resp = do_post(DDG_LITE_ENDPOINT).await.map_err(err)?;
                resp.text().await.map_err(err)
            }
            Err(_) => Err("DuckDuckGo request timed out".into()),
        }
    }
}

impl Default for DuckDuckGoBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn strip_tags(text: &str) -> String {
    let out = TAG_RE.replace_all(text, "");
    let decoded = html_decode(&out);
    WS_RE.replace_all(&decoded, " ").trim().to_string()
}

fn html_decode(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/")
}

/// Minimal percent-decoder. Invalid escapes are kept verbatim so we never
/// return empty on a weird edge case.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'+' {
            out.push(b' ');
            i += 1;
        } else if b == b'%' && i + 2 < bytes.len() {
            let h = &bytes[i + 1..i + 3];
            if let (Some(hi), Some(lo)) = (hex(h[0]), hex(h[1])) {
                out.push(hi * 16 + lo);
                i += 3;
            } else {
                out.push(b);
                i += 1;
            }
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Extract the real target URL from a DDG redirect (`//duckduckgo.com/l/?uddg=...`).
/// If the href is already absolute we return it unchanged.
fn unwrap_redirect(href: &str) -> String {
    if let Some(cap) = UDDG_RE.captures(href) {
        return percent_decode(&cap[1]);
    }
    if let Some(stripped) = href.strip_prefix("//") {
        return format!("https://{stripped}");
    }
    href.to_string()
}

fn parse_results(html: &str, max: usize) -> Vec<WebSearchItem> {
    let titles: Vec<(String, String)> = RESULT_A_RE
        .captures_iter(html)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect();
    let snippets: Vec<String> = SNIPPET_RE
        .captures_iter(html)
        .map(|c| c[1].to_string())
        .collect();
    titles
        .into_iter()
        .zip(snippets.into_iter().chain(std::iter::repeat(String::new())))
        .take(max)
        .map(|((href, title_html), snippet_html)| WebSearchItem {
            title: strip_tags(&title_html),
            url: unwrap_redirect(&href),
            snippet: strip_tags(&snippet_html),
        })
        .filter(|it| !it.url.is_empty())
        .collect()
}

#[async_trait]
impl WebSearchBackend for DuckDuckGoBackend {
    async fn search(&self, query: &str, count: u32) -> Result<Vec<WebSearchItem>, String> {
        let html = self.fetch_html(query).await?;
        let max = count.max(1) as usize;
        let items = parse_results(&html, max);
        if items.is_empty() {
            log::debug!("DDG parse returned 0 results (html len={})", html.len());
        }
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_basic() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(
            percent_decode("https%3A%2F%2Fexample.com%2Fx"),
            "https://example.com/x"
        );
        assert_eq!(percent_decode("bad%ZZ"), "bad%ZZ");
    }

    #[test]
    fn unwrap_redirect_variants() {
        assert_eq!(
            unwrap_redirect("//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F&rut=abc"),
            "https://example.com/"
        );
        assert_eq!(
            unwrap_redirect("https://example.com/direct"),
            "https://example.com/direct"
        );
        assert_eq!(
            unwrap_redirect("//example.com/path"),
            "https://example.com/path"
        );
    }

    #[test]
    fn parse_results_from_fixture() {
        // Minimal synthetic markup mirroring DDG's HTML structure.
        let html = r##"
<html><body>
<div class="result">
  <h2 class="result__title">
    <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fone&rut=x">Example <b>One</b></a>
  </h2>
  <a class="result__snippet" href="//dup">First result snippet &amp; more.</a>
</div>
<div class="result">
  <h2 class="result__title">
    <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org%2Ftwo">Example Two</a>
  </h2>
  <a class="result__snippet" href="//dup">Second snippet</a>
</div>
</body></html>
"##;
        let items = parse_results(html, 5);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].url, "https://example.com/one");
        assert_eq!(items[0].title, "Example One");
        assert!(items[0].snippet.contains("First result snippet"));
        assert!(items[0].snippet.contains("&"));
        assert_eq!(items[1].url, "https://example.org/two");
        assert_eq!(items[1].title, "Example Two");
    }

    #[test]
    fn parse_results_respects_max() {
        let html = r##"
<a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fa.com">A</a>
<a class="result__snippet">sa</a>
<a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fb.com">B</a>
<a class="result__snippet">sb</a>
<a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fc.com">C</a>
<a class="result__snippet">sc</a>
"##;
        assert_eq!(parse_results(html, 2).len(), 2);
    }
}
