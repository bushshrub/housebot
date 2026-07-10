//! Agent tool for fetching and summarizing a web page.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::llm::ChatClient;

const MAX_CONTENT_CHARS: usize = 8000;
const FETCH_TIMEOUT_SECS: u64 = 15;

/// OpenAI-style tool definition (internal `input_schema` form).
pub fn definition() -> Value {
    json!({
        "name": "summarize_url",
        "description": "Fetch the content of a public web page and return a concise summary. Use \
            this when the user shares a URL and wants to know what it contains, or when a search \
            result URL needs to be read in full.",
        "input_schema": {
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "The full URL (including https://) to fetch and summarize."}
            },
            "required": ["url"]
        }
    })
}

static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());
static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());

/// Strip HTML tags and collapse whitespace, truncating to the model's input budget.
pub fn strip_html(raw: &str) -> String {
    let no_tags = TAG_RE.replace_all(raw, " ");
    let collapsed = WS_RE.replace_all(no_tags.trim(), " ");
    let s = collapsed.trim();
    s.chars().take(MAX_CONTENT_CHARS).collect()
}

/// Summarize already-fetched HTML content via the LLM.
pub async fn summarize_content(
    client: &dyn ChatClient,
    model: &str,
    url: &str,
    raw_html: &str,
) -> String {
    let content = strip_html(raw_html);
    let prompt = format!(
        "Summarize the following web page content in 3-5 sentences. Focus on the most important \
         information.\n\nURL: {url}\n\nCONTENT:\n{content}"
    );
    let messages = vec![json!({"role": "user", "content": prompt})];
    match client.chat_once(model, &messages, 512).await {
        Ok(out) if !out.is_empty() => out,
        _ => "(no summary generated)".to_string(),
    }
}

/// Fetch `url` and summarize it, returning an `Error:` string on any fetch failure.
pub async fn fetch_and_summarize(client: &dyn ChatClient, model: &str, url: &str) -> String {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .user_agent("house-chatbot/1.0")
        .build();
    let http = match http {
        Ok(c) => c,
        Err(e) => return format!("Error: could not build HTTP client: {e}"),
    };
    let resp = match http.get(url).send().await {
        Ok(r) => r,
        Err(e) => return format!("Error: could not fetch URL: {e}"),
    };
    if !resp.status().is_success() {
        return format!("Error: HTTP {} when fetching {url}", resp.status().as_u16());
    }
    let raw = match resp.text().await {
        Ok(t) => t,
        Err(e) => return format!("Error: could not read response body: {e}"),
    };
    summarize_content(client, model, url, &raw).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockChatClient;

    #[test]
    fn strip_html_removes_tags_and_collapses_ws() {
        let out = strip_html("<html>  <body>cats   content</body>\n</html>");
        assert_eq!(out, "cats content");
    }

    #[test]
    fn strip_html_truncates() {
        let raw = "x".repeat(MAX_CONTENT_CHARS + 500);
        assert_eq!(strip_html(&raw).chars().count(), MAX_CONTENT_CHARS);
    }

    #[tokio::test]
    async fn summarize_content_returns_llm_summary() {
        let client = MockChatClient::new().with_once_reply("This page is about cats.");
        let out = summarize_content(&client, "m", "https://example.com", "<b>cats</b>").await;
        assert_eq!(out, "This page is about cats.");
    }

    #[tokio::test]
    async fn summarize_content_prompt_includes_url_and_content() {
        let client = MockChatClient::new().with_once_reply("ok");
        summarize_content(&client, "m", "https://example.com/x", "<p>hello world</p>").await;
        let calls = client.once_calls.lock().unwrap();
        let content = calls[0][0]["content"].as_str().unwrap();
        assert!(content.contains("https://example.com/x"));
        assert!(content.contains("hello world"));
    }

    #[tokio::test]
    async fn summarize_content_empty_returns_fallback() {
        let client = MockChatClient::new();
        let out = summarize_content(&client, "m", "u", "<p>x</p>").await;
        assert!(out.contains("no summary"));
    }

    #[test]
    fn definition_requires_url() {
        let d = definition();
        assert_eq!(d["name"], "summarize_url");
        assert_eq!(d["input_schema"]["required"], json!(["url"]));
    }
}
