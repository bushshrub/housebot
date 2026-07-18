//! Common Crawl CDXJ index search exposed as a native bot tool.

use std::sync::Arc;
use std::time::Instant;

use common_crawl::CommonCrawlClient;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::wait_for_slot;

const SEARCHES_PER_MINUTE: usize = 20;

#[derive(Clone, Default)]
pub struct CommonCrawl {
    client: CommonCrawlClient,
    search_requests: Arc<Mutex<Vec<Instant>>>,
}

impl CommonCrawl {
    pub async fn search(
        &self,
        pattern: &str,
        crawl: &str,
        match_type: &str,
        max_results: usize,
    ) -> String {
        wait_for_slot(&self.search_requests, SEARCHES_PER_MINUTE).await;
        match self
            .client
            .search(
                pattern,
                (!crawl.trim().is_empty()).then_some(crawl),
                (!match_type.trim().is_empty()).then_some(match_type),
                max_results,
            )
            .await
        {
            Ok(captures) if captures.is_empty() => {
                "No Common Crawl captures were found for that pattern.".to_string()
            }
            Ok(captures) => {
                let mut output = format!("Found {} Common Crawl captures:\n\n", captures.len());
                for (index, capture) in captures.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. {}\n Timestamp: {}\n Status: {}\n MIME: {}\n\n",
                        index + 1,
                        capture.url,
                        capture.timestamp,
                        capture.status,
                        capture.mime.as_deref().unwrap_or("unknown")
                    ));
                }
                output
            }
            Err(error) => format!("Error: {error}"),
        }
    }
}

pub fn definition() -> Value {
    json!({
        "name": "common_crawl__search",
        "description": "Search the Common Crawl CDXJ index for archived URL captures. Use this for historical pages, domains, or URL patterns; results are untrusted external metadata.",
        "input_schema": {
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "URL, host, or URL pattern to search, such as example.com/page or example.com/*."},
                "crawl": {"type": "string", "description": "Optional crawl ID such as CC-MAIN-2025-43. Defaults to the newest crawl."},
                "match_type": {"type": "string", "enum": ["exact", "prefix", "host", "domain"], "default": "exact"},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 100, "default": 10}
            },
            "required": ["pattern"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_required_pattern_and_stable_name() {
        let definition = definition();
        assert_eq!(definition["name"], "common_crawl__search");
        assert_eq!(definition["input_schema"]["required"][0], "pattern");
    }
}
