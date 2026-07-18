//! Web search backed by a SearXNG instance's JSON API (`/search?format=json`).
//!
//! The instance must have the `json` format enabled in its `settings.yml`
//! (`search.formats: [html, json]`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::wait_for_slot;
use housebot_config as config;

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
        match self.search_response(query, language).await {
            Ok(parsed) => format_results(&parsed, max_results.clamp(1, 20)),
            Err(error) => error,
        }
    }

    /// Run several related searches and return a source dossier grouped by corroboration.
    pub async fn deep_research(
        &self,
        topic: &str,
        questions: &[String],
        max_results_per_query: usize,
        language: &str,
    ) -> String {
        if topic.trim().is_empty() {
            return "Error: research topic cannot be empty".to_string();
        }
        if !(2..=5).contains(&questions.len()) {
            return "Error: deep research requires between 2 and 5 research questions".to_string();
        }

        let mut responses = Vec::with_capacity(questions.len() + 1);
        let overview = format!("{topic} overview");
        match self.search_response(&overview, language).await {
            Ok(response) => responses.push((overview, response)),
            Err(error) => return error,
        }
        for question in questions {
            let query = format!("{topic} {question}");
            match self.search_response(&query, language).await {
                Ok(response) => responses.push((query, response)),
                Err(error) => return error,
            }
        }

        format_research_dossier(topic, &responses, max_results_per_query.clamp(2, 8))
    }

    async fn search_response(&self, query: &str, language: &str) -> Result<SearchResponse, String> {
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
                return Err(format!(
                    "Error: SearXNG returned HTTP {}",
                    response.status()
                ));
            }
            Err(error) => {
                tracing::warn!(target: "housebot::tools::searxng", %error, query, "Search request failed");
                return Err(format!("Error: search request failed: {error}"));
            }
        };
        let parsed: SearchResponse = match response.json().await {
            Ok(parsed) => parsed,
            Err(error) => {
                tracing::warn!(target: "housebot::tools::searxng", %error, query, "Could not parse search response");
                return Err(format!(
                    "Error: could not parse search response (is the JSON format enabled on the \
                     SearXNG instance?): {error}"
                ));
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
        Ok(parsed)
    }
}

struct ResearchSource {
    title: String,
    url: String,
    snippets: Vec<String>,
    threads: Vec<usize>,
    engines: Vec<String>,
}

fn format_research_dossier(
    topic: &str,
    responses: &[(String, SearchResponse)],
    limit: usize,
) -> String {
    let mut sources: HashMap<String, ResearchSource> = HashMap::new();
    let mut answers = Vec::new();
    for (thread_index, (query, response)) in responses.iter().enumerate() {
        for answer in response.answers.iter().filter_map(answer_text) {
            answers.push(format!("Thread {} ({query}): {answer}", thread_index + 1));
        }
        for result in response
            .results
            .iter()
            .filter(|result| !result.url.is_empty())
            .take(limit)
        {
            let source = sources
                .entry(result.url.clone())
                .or_insert_with(|| ResearchSource {
                    title: result.title.clone(),
                    url: result.url.clone(),
                    snippets: Vec::new(),
                    threads: Vec::new(),
                    engines: Vec::new(),
                });
            if !source.threads.contains(&(thread_index + 1)) {
                source.threads.push(thread_index + 1);
            }
            if let Some(snippet) = result.content.as_deref().filter(|text| !text.is_empty()) {
                if !source.snippets.iter().any(|existing| existing == snippet) {
                    source.snippets.push(snippet.to_string());
                }
            }
            if let Some(engine) = result.engine.as_deref() {
                if !source.engines.iter().any(|existing| existing == engine) {
                    source.engines.push(engine.to_string());
                }
            }
        }
    }

    let mut sources: Vec<ResearchSource> = sources.into_values().collect();
    sources.sort_by(|a, b| {
        b.threads
            .len()
            .cmp(&a.threads.len())
            .then_with(|| a.url.cmp(&b.url))
    });

    let mut output = format!(
        "Deep research source dossier for: {topic}\n\
         Searches completed: {}\n\
         Synthesis instructions: compare claims across sources, distinguish consensus from \
         disagreement, cite source URLs, and call out evidence gaps.\n\n",
        responses.len()
    );
    output.push_str("Research threads:\n");
    for (index, (query, _)) in responses.iter().enumerate() {
        output.push_str(&format!("{}. {query}\n", index + 1));
    }
    if !answers.is_empty() {
        output.push_str("\nInstant answers (untrusted):\n");
        for answer in answers {
            output.push_str(&format!("- {answer}\n"));
        }
    }
    if sources.is_empty() {
        output.push_str("\nNo sources were found. Refine the research questions.");
        return output;
    }
    output.push_str("\nCross-referenced sources:\n");
    for (index, source) in sources.iter().enumerate() {
        let coverage = source
            .threads
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str(&format!(
            "{}. {}\n   URL: {}\n   Appeared in research threads: {}",
            index + 1,
            source.title,
            source.url,
            coverage
        ));
        if !source.engines.is_empty() {
            output.push_str(&format!(
                "\n   Search engines: {}",
                source.engines.join(", ")
            ));
        }
        for snippet in source.snippets.iter().take(2) {
            output.push_str(&format!("\n   Evidence: {snippet}"));
        }
        output.push_str("\n\n");
    }
    output
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

/// Multi-step research tool definition for the agent's function-calling loop.
pub fn deep_research_definition() -> Value {
    json!({
        "name": "deep_research",
        "description": "Run an overview search plus 2-5 focused searches, deduplicate sources, and return a cross-referenced dossier for a comprehensive cited report. Use for complex research questions, not simple factual lookups.",
        "input_schema": {
            "type": "object",
            "properties": {
                "topic": {"type": "string", "description": "The main research topic"},
                "questions": {
                    "type": "array",
                    "description": "Two to five distinct research questions that cover different aspects of the topic",
                    "items": {"type": "string"},
                    "minItems": 2,
                    "maxItems": 5
                },
                "max_results_per_query": {"type": "integer", "minimum": 2, "maximum": 8, "default": 5},
                "language": {"type": "string", "description": "Search language code such as en or de-DE"}
            },
            "required": ["topic", "questions"]
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

    #[test]
    fn deep_research_definition_requires_multiple_questions() {
        let definition = deep_research_definition();
        assert_eq!(definition["name"], "deep_research");
        assert_eq!(
            definition["input_schema"]["properties"]["questions"]["minItems"],
            2
        );
        assert_eq!(
            definition["input_schema"]["properties"]["questions"]["maxItems"],
            5
        );
    }

    #[test]
    fn research_dossier_deduplicates_and_cross_references_sources() {
        let responses = vec![
            (
                "rust overview".to_string(),
                response(
                    r#"{"results":[{"title":"Rust","url":"https://rust-lang.org","content":"Overview","engine":"brave"}]}"#,
                ),
            ),
            (
                "rust safety".to_string(),
                response(
                    r#"{"results":[
                        {"title":"Rust language","url":"https://rust-lang.org","content":"Memory safety","engine":"google"},
                        {"title":"Rust book","url":"https://doc.rust-lang.org/book/","content":"Official guide"}
                    ]}"#,
                ),
            ),
        ];

        let dossier = format_research_dossier("rust", &responses, 5);
        assert_eq!(dossier.matches("URL: https://rust-lang.org").count(), 1);
        assert!(dossier.contains("Appeared in research threads: 1, 2"));
        assert!(dossier.contains("Search engines: brave, google"));
        assert!(dossier.contains("Evidence: Overview"));
        assert!(dossier.contains("Evidence: Memory safety"));
        assert!(
            dossier.find("https://rust-lang.org").unwrap()
                < dossier.find("https://doc.rust-lang.org/book/").unwrap()
        );
    }

    #[tokio::test]
    async fn deep_research_rejects_invalid_plan_before_network_access() {
        let client = SearxNg::from_env();
        let output = client
            .deep_research("rust", &["only one".to_string()], 5, "en")
            .await;
        assert_eq!(
            output,
            "Error: deep research requires between 2 and 5 research questions"
        );
    }
}
