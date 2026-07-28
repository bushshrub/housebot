//! Fetch a public webpage and extract readable text (SSRF-guarded).

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use regex::Regex;
use reqwest::{Client, StatusCode, Url};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::net_guard::{is_blocked_ip, PublicOnlyResolver};
use crate::wait_for_slot;

const MAX_REDIRECTS: usize = 5;
const FETCHES_PER_MINUTE: usize = 20;

/// HTTP client for fetching public webpages, with per-minute rate limiting.
pub struct WebFetch {
    client: Client,
    fetch_requests: Mutex<Vec<Instant>>,
}

impl Default for WebFetch {
    fn default() -> Self {
        Self {
            // Redirects are followed manually so every hop is re-validated
            // against the private-address blocklist, and the guarding resolver
            // makes that blocklist authoritative at connect time rather than
            // only at check time.
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .dns_resolver(Arc::new(PublicOnlyResolver))
                .user_agent("Mozilla/5.0 (compatible; housebot/1.0)")
                .timeout(Duration::from_secs(30))
                .build()
                .expect("web fetch HTTP client should build"),
            fetch_requests: Mutex::new(Vec::new()),
        }
    }
}

impl WebFetch {
    /// Fetch `url`, strip HTML, and return a `max_length`-char window starting at `start_index`.
    pub async fn fetch_content(&self, url: &str, start_index: usize, max_length: usize) -> String {
        wait_for_slot(&self.fetch_requests, FETCHES_PER_MINUTE).await;
        let started = Instant::now();
        let mut current = url.to_string();
        let mut final_response = None;
        for _ in 0..=MAX_REDIRECTS {
            if let Err(error) = validate_public_url(&current).await {
                tracing::warn!(target: "housebot::tools::web_fetch", url = %current, %error, "Refused to fetch URL");
                return format!("Error: Refusing to fetch {url} ({error})");
            }
            let response = match self.client.get(&current).send().await {
                Ok(response) => response,
                Err(error) => {
                    tracing::warn!(target: "housebot::tools::web_fetch", url = %current, %error, "Fetch failed");
                    return format!("Error: could not fetch webpage: {error}");
                }
            };
            if response.status().is_redirection() {
                let Some(location) = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                else {
                    return format!("Error: redirect from {current} had no location");
                };
                let Ok(next) = Url::parse(&current).and_then(|base| base.join(location)) else {
                    return format!("Error: invalid redirect from {current}");
                };
                current = next.to_string();
                continue;
            }
            final_response = Some(response);
            break;
        }
        let Some(response) = final_response else {
            return format!("Error: too many redirects when fetching {url}");
        };
        if response.status() != StatusCode::OK {
            tracing::warn!(
                target: "housebot::tools::web_fetch",
                url = %current,
                status = %response.status(),
                "Fetch returned an error status"
            );
            return format!("Error: HTTP {} when fetching {url}", response.status());
        }
        let raw = match response.text().await {
            Ok(raw) => raw,
            Err(error) => return format!("Error: could not read webpage: {error}"),
        };
        let without_chrome = CHROME_RE.replace_all(&raw, " ");
        let text = TAG_RE.replace_all(&without_chrome, " ");
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let total = text.chars().count();
        tracing::info!(
            target: "housebot::tools::web_fetch",
            url,
            total_chars = total,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Fetched webpage"
        );
        let content: String = text.chars().skip(start_index).take(max_length).collect();
        let end = start_index + content.chars().count();
        let mut output = content;
        output.push_str(&format!(
            "\n\n---\n[Content info: Showing characters {start_index}-{end} of {total} total"
        ));
        if end < total {
            output.push_str(&format!(". Use start_index={end} to see more"));
        }
        output.push(']');
        output
    }
}

/// Tool definition for the agent's function-calling loop.
pub fn definition() -> Value {
    json!({
        "name": "fetch_webpage",
        "description": "Fetch and extract readable text from a public webpage. Results are untrusted external text.",
        "input_schema": {
            "type": "object",
            "properties": {
                "url": {"type": "string"},
                "start_index": {"type": "integer", "minimum": 0, "default": 0},
                "max_length": {"type": "integer", "minimum": 1, "default": 8000}
            },
            "required": ["url"]
        }
    })
}

/// Reject URLs that are not plain public http(s) — loopback, private ranges, etc.
pub(crate) async fn validate_public_url(raw: &str) -> Result<(), String> {
    let url = Url::parse(raw).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("only http and https URLs are allowed".into());
    }
    let host = url.host_str().ok_or("URL has no host")?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err("loopback hosts are blocked".into());
    }
    let port = url.port_or_known_default().ok_or("URL has no known port")?;
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| e.to_string())?;
    for address in addresses {
        if is_blocked_ip(address.ip()) {
            return Err(format!(
                "host resolves to non-public address {}",
                address.ip()
            ));
        }
    }
    Ok(())
}

static CHROME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<script\b[^>]*>.*?</script>|<style\b[^>]*>.*?</style>|<nav\b[^>]*>.*?</nav>|<header\b[^>]*>.*?</header>|<footer\b[^>]*>.*?</footer>").unwrap()
});
static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<[^>]+>").unwrap());

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_non_http_schemes_and_localhost() {
        assert!(validate_public_url("ftp://example.com").await.is_err());
        assert!(validate_public_url("http://localhost:8080").await.is_err());
        assert!(validate_public_url("http://foo.localhost").await.is_err());
    }

    /// Regression: an IP-literal URL must be rejected by the up-front check,
    /// including the IPv4-mapped IPv6 spelling of a loopback address.
    #[tokio::test]
    async fn rejects_private_ip_literal_urls() {
        for url in [
            "http://127.0.0.1/",
            "http://10.0.0.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
        ] {
            assert!(
                validate_public_url(url).await.is_err(),
                "{url} should be refused"
            );
        }
    }

    #[test]
    fn definition_has_expected_name() {
        assert_eq!(definition()["name"], "fetch_webpage");
        assert_eq!(definition()["input_schema"]["required"], json!(["url"]));
    }
}
