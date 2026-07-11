//! Small async client for Common Crawl's CDXJ index service.

use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};

const INDEX_URL: &str = "https://index.commoncrawl.org";
const COLLECTIONS_URL: &str = "https://index.commoncrawl.org/collinfo.json";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Capture {
    pub url: String,
    pub timestamp: String,
    pub status: String,
    pub mime: Option<String>,
    pub digest: Option<String>,
    pub length: Option<String>,
    pub offset: Option<String>,
    pub filename: Option<String>,
    #[serde(rename = "mime-detected")]
    pub mime_detected: Option<String>,
    pub languages: Option<String>,
    pub encoding: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Collection {
    id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Common Crawl request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Common Crawl returned HTTP {status}: {body}")]
    Http {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("Common Crawl returned invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Common Crawl returned no collections")]
    NoCollections,
    #[error("URL pattern cannot be empty")]
    EmptyPattern,
}

#[derive(Clone)]
pub struct CommonCrawlClient {
    http: Client,
    index_url: String,
    collections_url: String,
}

impl Default for CommonCrawlClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CommonCrawlClient {
    pub fn new() -> Self {
        Self::with_endpoints(INDEX_URL, COLLECTIONS_URL)
    }

    /// Construct a client with custom endpoints, primarily for integration tests.
    pub fn with_endpoints(
        index_url: impl Into<String>,
        collections_url: impl Into<String>,
    ) -> Self {
        Self {
            http: Client::builder()
                .user_agent("housebot-common-crawl/0.1")
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Common Crawl HTTP client should build"),
            index_url: index_url.into().trim_end_matches('/').to_string(),
            collections_url: collections_url.into(),
        }
    }

    pub async fn latest_collection(&self) -> Result<String, Error> {
        let response = self.http.get(&self.collections_url).send().await?;
        let body = response_body(response).await?;
        let collections: Vec<Collection> = serde_json::from_str(&body)?;
        collections
            .first()
            .map(|collection| collection.id.clone())
            .ok_or(Error::NoCollections)
    }

    pub async fn search(
        &self,
        pattern: &str,
        crawl: Option<&str>,
        match_type: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Capture>, Error> {
        if pattern.trim().is_empty() {
            return Err(Error::EmptyPattern);
        }
        let crawl = match crawl.filter(|value| !value.trim().is_empty()) {
            Some(crawl) => crawl.to_string(),
            None => self.latest_collection().await?,
        };
        let mut request = self
            .http
            .get(format!("{}/{}-index", self.index_url, crawl))
            .query(&[("url", pattern), ("output", "json")])
            .query(&[("pageSize", &limit.clamp(1, 100).to_string())]);
        if let Some(match_type) = match_type.filter(|value| !value.trim().is_empty()) {
            request = request.query(&[("matchType", match_type)]);
        }
        let response = request.send().await?;
        let body = response_body(response).await?;
        body.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| Ok(serde_json::from_str(line)?))
            .collect()
    }
}

async fn response_body(response: reqwest::Response) -> Result<String, Error> {
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(Error::Http {
            status,
            body: body.chars().take(500).collect(),
        });
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_patterns_are_rejected() {
        let client = CommonCrawlClient::new();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let error = runtime.block_on(client.search("  ", Some("CC-MAIN-test"), None, 10));
        assert!(matches!(error, Err(Error::EmptyPattern)));
    }

    #[test]
    fn capture_fields_match_cdxj_names() {
        let capture: Capture = serde_json::from_str(
            r#"{"url":"https://example.com","timestamp":"20250101000000","status":"200","mime":"text/html","mime-detected":"text/html","digest":"abc","length":"12","offset":"3","filename":"crawl/file.warc.gz"}"#,
        )
        .unwrap();
        assert_eq!(capture.url, "https://example.com");
        assert_eq!(capture.mime_detected.as_deref(), Some("text/html"));
    }
}
