//! Client for a System One decision endpoint (`POST /v1/systemone`).
//!
//! A decision model reads one document (the state) and answers typed
//! questions with a probability for each option. It generates no text, so a
//! call costs one forward pass.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

pub struct SystemOneClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Answer {
    pub choice: String,
    #[serde(default)]
    pub probabilities: HashMap<String, f64>,
}

impl Answer {
    pub fn probability(&self, option: &str) -> f64 {
        self.probabilities.get(option).copied().unwrap_or(0.0)
    }
}

#[derive(Deserialize)]
struct Response {
    answers: HashMap<String, Answer>,
}

impl SystemOneClient {
    /// Build a client for `base_url`, with or without a trailing `/v1`
    /// (e.g. `https://llm.example.net/typesafe`).
    pub fn new(base_url: &str, api_key: impl Into<String>) -> Self {
        let base_url = base_url.trim_end_matches('/');
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.strip_suffix("/v1").unwrap_or(base_url).to_string(),
            api_key: api_key.into(),
        }
    }

    /// Ask `questions` (a JSON object keyed by question name) about `state`.
    pub async fn decide(
        &self,
        model: &str,
        state: &str,
        questions: &Value,
    ) -> anyhow::Result<HashMap<String, Answer>> {
        let response = self
            .http
            .post(format!("{}/v1/systemone", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&json!({"model": model, "state": state, "questions": questions}))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "System One returned {status}: {}",
                body.chars().take(300).collect::<String>()
            );
        }
        Ok(response.json::<Response>().await?.answers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_accepts_a_trailing_v1() {
        for url in [
            "https://llm.example.net/typesafe",
            "https://llm.example.net/typesafe/",
            "https://llm.example.net/typesafe/v1",
        ] {
            assert_eq!(
                SystemOneClient::new(url, "").base_url,
                "https://llm.example.net/typesafe"
            );
        }
    }

    #[test]
    fn parses_a_choice_answer() {
        let body = r#"{"model":"kev-4b","answers":{"team":{"type":"choice","choice":"billing",
            "probabilities":{"billing":0.94,"returns":0.06},"confidence":0.92}},
            "usage":{"input_tokens":47,"output_tokens":0}}"#;
        let response: Response = serde_json::from_str(body).unwrap();
        let answer = &response.answers["team"];
        assert_eq!(answer.choice, "billing");
        assert_eq!(answer.probability("returns"), 0.06);
        assert_eq!(answer.probability("missing"), 0.0);
    }
}
