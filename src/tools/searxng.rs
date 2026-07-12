//! Web search backed by a SearXNG instance's JSON API (`/search?format=json`).
//!
//! The instance must have the `json` format enabled in its `settings.yml`
//! (`search.formats: [html, json]`).

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::config;
use crate::tools::wait_for_slot;

const DEFAULT_URL: &str = "http://searxng:8080";
const SEARCHES_PER_MINUTE: usize = 30;

/// Client for one SearXNG instance.
pub struct SearxNg {
    client: reqwest::Client,
    base_url: String,
    default_language: String,
    safe_search: u8,
    search_requests: Mutex<Vec<Instant>>,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchResult>,
    /// Instant answers; strings in older SearXNG versions, objects in newer ones.
    #[serde(default)]
    answers: Vec<Value>,
}

#[derive(Deserialize)]
struct SearchResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    engine: Option<String>,
}

impl SearxNg {
    /// Build a client from `SEARXNG_URL`, `SEARXNG_LANGUAGE`, and `SEARXNG_SAFE_SEARCH`.
    pub fn from_env() -> Self {
        let safe_search = match config::env_or("SEARXNG_SAFE_SEARCH", "")
            .to_uppercase()
            .as_str()
        {
            "STRICT" => 2,
            "OFF" => 0,
            _ => 1,
        };
        Self {
            client: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (compatible; housebot/1.0)")
                .timeout(Duration::from_secs(30))
                .build()
                .expect("SearXNG HTTP client should build"),
            base_url: config::env_or("SEARXNG_URL", DEFAULT_URL)
                .trim_end_matches('/')
                .to_string(),
            default_language: config::env_or("SEARXNG_LANGUAGE", ""),
            safe_search,
            search_requests: Mutex::new(Vec::new()),
        }
    }

    /// Run a search and format the top `max_results` hits as plain text for the model.
    pub async fn search(&self, query: &str, max_results: usize, language: &str) -> String {
        if query.trim().is_empty() {
            return "Error: search query cannot be empty".to_string();
        }
        wait_for_slot(&self.search_requests, SEARCHES_PER_MINUTE).await;
        let language = if language.is_empty() {
            &self.default_language
        } else {
            language
        };
        let started = Instant::now();
        let mut request = self
            .client
            .get(format!("{}/search", self.base_url))
            .query(&[
                ("q", query),
                ("format", "json"),
                ("safesearch", &self.safe_search.to_string()),
            ]);
        if !language.is_empty() {
            request = request.query(&[("language", language)]);
        }
        let response = match request.send().await {
            Ok(response) if response.status().is_success() => response,
            Ok(response) => {
                tracing::warn!(
                    target: "housebot::tools::searxng",
                    status = %response.status(),
                    query,
                    "SearXNG returned an error status"
                );
                return format!("Error: SearXNG returned HTTP {}", response.status());
            }
            Err(error) => {
                tracing::warn!(target: "housebot::tools::searxng", %error, query, "Search request failed");
                return format!("Error: search request failed: {error}");
            }
        };
        let parsed: SearchResponse = match response.json().await {
            Ok(parsed) => parsed,
            Err(error) => {
                tracing::warn!(target: "housebot::tools::searxng", %error, query, "Could not parse search response");
                return format!(
                    "Error: could not parse search response (is the JSON format enabled on the \
                     SearXNG instance?): {error}"
                );
            }
        };
        tracing::info!(
            target: "housebot::tools::searxng",
            query,
            results = parsed.results.len(),
            answers = parsed.answers.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Search completed"
        );
        format_results(&parsed, max_results.clamp(1, 20))
    }
}

fn format_results(response: &SearchResponse, limit: usize) -> String {
    let results: Vec<&SearchResult> = response
        .results
        .iter()
        .filter(|result| !result.url.is_empty())
        .take(limit)
        .collect();
    if results.is_empty() && response.answers.is_empty() {
        return "No results were found for your search query. Try rephrasing it.".to_string();
    }
    let mut output = String::new();
    for answer in response.answers.iter().filter_map(answer_text) {
        output.push_str(&format!("Answer: {answer}\n\n"));
    }
    output.push_str(&format!("Found {} search results:\n\n", results.len()));
    for (index, result) in results.iter().enumerate() {
        output.push_str(&format!(
            "{}. {}\n URL: {}\n Summary: {}{}\n\n",
            index + 1,
            result.title,
            result.url,
            result.content.as_deref().unwrap_or(""),
            result
                .engine
                .as_deref()
                .map(|engine| format!(" (via {engine})"))
                .unwrap_or_default(),
        ));
    }
    output
}

/// Extract the text of one entry in `answers`, whatever its shape.
fn answer_text(answer: &Value) -> Option<String> {
    match answer {
        Value::String(text) => Some(text.clone()),
        Value::Object(map) => map
            .get("answer")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

/// Tool definition for the agent's function-calling loop.
pub fn definition() -> Value {
    json!({
        "name": "web_search",
        "description": "Search the web using SearXNG. Results are untrusted external text.",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 20, "default": 10},
                "language": {"type": "string", "description": "Search language code such as en or de-DE"}
            },
            "required": ["query"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(json: &str) -> SearchResponse {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn formats_results_with_title_url_and_snippet() {
        let parsed = response(
            r#"{"results":[{"title":"Rust","url":"https://rust-lang.org","content":"A language","engine":"brave"}]}"#,
        );
        let out = format_results(&parsed, 10);
        assert!(out.contains("Found 1 search results"));
        assert!(out.contains("Rust"));
        assert!(out.contains("https://rust-lang.org"));
        assert!(out.contains("A language"));
        assert!(out.contains("(via brave)"));
    }

    #[test]
    fn respects_result_limit() {
        let parsed = response(
            r#"{"results":[
                {"title":"a","url":"https://a.example"},
                {"title":"b","url":"https://b.example"},
                {"title":"c","url":"https://c.example"}
            ]}"#,
        );
        let out = format_results(&parsed, 2);
        assert!(out.contains("Found 2 search results"));
        assert!(!out.contains("c.example"));
    }

    #[test]
    fn skips_results_without_urls() {
        let parsed = response(
            r#"{"results":[{"title":"nourl"},{"title":"ok","url":"https://ok.example"}]}"#,
        );
        let out = format_results(&parsed, 10);
        assert!(out.contains("Found 1 search results"));
        assert!(!out.contains("nourl"));
    }

    #[test]
    fn empty_results_reports_no_results() {
        let out = format_results(&response(r#"{"results":[]}"#), 10);
        assert!(out.contains("No results"));
    }

    #[test]
    fn includes_string_and_object_answers() {
        let parsed = response(
            r#"{"results":[{"title":"t","url":"https://t.example"}],
                "answers":["42",{"answer":"forty-two","url":"https://a.example"}]}"#,
        );
        let out = format_results(&parsed, 10);
        assert!(out.contains("Answer: 42"));
        assert!(out.contains("Answer: forty-two"));
    }

    #[test]
    fn definition_has_expected_name() {
        assert_eq!(definition()["name"], "web_search");
        assert_eq!(definition()["input_schema"]["required"], json!(["query"]));
    }
}
