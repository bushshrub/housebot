//! Firecrawl's keyless scrape endpoint, used only when local extraction of a
//! page comes back empty (typically a page rendered by JavaScript).
//!
//! The keyless tier is shared per IP at 1,000 credits a month; the bot keeps
//! itself to [`MONTHLY_CREDIT_LIMIT`] of them with a counter in PostgreSQL.
//! Only `/v2/scrape` (1 credit per page) is ever called.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::web_fetch::Page;

const SCRAPE_URL: &str = "https://api.firecrawl.dev/v2/scrape";
pub const MONTHLY_CREDIT_LIMIT: i32 = 200;

/// Keyless Firecrawl client with a persistent monthly credit budget.
pub struct Firecrawl {
    client: reqwest::Client,
    db: Arc<tokio_postgres::Client>,
}

#[derive(Deserialize)]
struct ScrapeResponse {
    #[serde(default)]
    data: Option<ScrapeData>,
}

#[derive(Deserialize)]
struct ScrapeData {
    #[serde(default)]
    markdown: String,
    #[serde(default)]
    metadata: ScrapeMetadata,
}

#[derive(Default, Deserialize)]
struct ScrapeMetadata {
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "statusCode")]
    status_code: Option<u16>,
}

impl Firecrawl {
    pub fn new(db: Arc<tokio_postgres::Client>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("Firecrawl HTTP client should build"),
            db,
        }
    }

    /// Scrape `url`, or `None` when the budget is spent or the scrape fails.
    pub(crate) async fn scrape(&self, url: &str) -> Option<Page> {
        match self.reserve_credit().await {
            Ok(true) => {}
            Ok(false) => {
                tracing::info!(
                    target: "housebot::tools::firecrawl",
                    limit = MONTHLY_CREDIT_LIMIT,
                    "Firecrawl monthly credit limit reached; using local extraction only"
                );
                return None;
            }
            Err(error) => {
                tracing::warn!(target: "housebot::tools::firecrawl", %error, "Could not reserve a Firecrawl credit");
                return None;
            }
        }
        let response = self
            .client
            .post(SCRAPE_URL)
            .json(&json!({
                "url": url,
                "formats": ["markdown"],
                "onlyMainContent": true,
                // A fresh scrape, kept out of Firecrawl's shared cache and index.
                "maxAge": 0,
                "storeInCache": false,
                "skipTlsVerification": false,
            }))
            .send()
            .await;
        let response = match response {
            Ok(response) if response.status().is_success() => response,
            Ok(response) => {
                tracing::warn!(target: "housebot::tools::firecrawl", url, status = %response.status(), "Firecrawl scrape was rejected");
                return None;
            }
            Err(error) => {
                tracing::warn!(target: "housebot::tools::firecrawl", url, %error, "Firecrawl scrape failed");
                return None;
            }
        };
        match response.json::<ScrapeResponse>().await {
            Ok(parsed) => {
                let page = page_from_response(parsed);
                tracing::info!(
                    target: "housebot::tools::firecrawl",
                    url,
                    chars = page.as_ref().map_or(0, |page| page.text.chars().count()),
                    "Firecrawl scrape completed"
                );
                page
            }
            Err(error) => {
                tracing::warn!(target: "housebot::tools::firecrawl", url, %error, "Could not parse the Firecrawl response");
                None
            }
        }
    }

    /// Count one credit against this month, refusing once the limit is reached.
    /// The check and the increment are one statement, so concurrent fetches
    /// cannot overshoot the limit.
    async fn reserve_credit(&self) -> Result<bool, tokio_postgres::Error> {
        let row = self
            .db
            .query_opt(
                "INSERT INTO firecrawl_usage (month, credits)
                 VALUES (date_trunc('month', NOW() AT TIME ZONE 'UTC')::date, 1)
                 ON CONFLICT (month) DO UPDATE
                     SET credits = firecrawl_usage.credits + 1, updated_at = NOW()
                     WHERE firecrawl_usage.credits < $1
                 RETURNING credits",
                &[&MONTHLY_CREDIT_LIMIT],
            )
            .await?;
        Ok(row.is_some())
    }
}

fn page_from_response(response: ScrapeResponse) -> Option<Page> {
    let data = response.data?;
    if data
        .metadata
        .status_code
        .is_some_and(|status| !(200..300).contains(&status))
    {
        return None;
    }
    let text = data.markdown.trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some(Page {
        title: data.metadata.title.unwrap_or_default(),
        text,
        source: "Firecrawl",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Option<Page> {
        page_from_response(serde_json::from_str(json).unwrap())
    }

    #[test]
    fn scrape_response_yields_markdown_and_title() {
        let page = parse(
            r##"{"success":true,"data":{"markdown":"# Hello\n\nBody","metadata":{"title":"Hello page","statusCode":200}}}"##,
        )
        .unwrap();
        assert_eq!(page.title, "Hello page");
        assert_eq!(page.text, "# Hello\n\nBody");
        assert_eq!(page.source, "Firecrawl");
    }

    #[test]
    fn error_pages_and_empty_scrapes_are_discarded() {
        assert!(
            parse(r#"{"data":{"markdown":"Not found","metadata":{"statusCode":404}}}"#).is_none()
        );
        assert!(parse(r#"{"data":{"markdown":"  ","metadata":{}}}"#).is_none());
        assert!(parse(r#"{"success":false}"#).is_none());
    }
}
