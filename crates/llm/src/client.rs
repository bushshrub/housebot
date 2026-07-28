//! Client.

use crate::stream::*;
use crate::*;
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::Value;

/// Real HTTP client against an OpenAI-compatible server.
pub struct OpenAiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl OpenAiClient {
    /// Build a client for `base_url` (e.g. `http://server-slop:8080/v1`).
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    fn props_endpoint(&self) -> String {
        let root = self.base_url.strip_suffix("/v1").unwrap_or(&self.base_url);
        format!("{root}/props")
    }
}

#[derive(Deserialize)]
struct PropsResponse {
    default_generation_settings: DefaultGenerationSettings,
}

#[derive(Deserialize)]
struct DefaultGenerationSettings {
    n_ctx: u64,
}

#[async_trait]
impl ChatClient for OpenAiClient {
    async fn context_window_tokens(&self) -> anyhow::Result<Option<u64>> {
        let response = self
            .http
            .get(self.props_endpoint())
            .bearer_auth(&self.api_key)
            .send()
            .await?
            .error_for_status()?
            .json::<PropsResponse>()
            .await?;
        Ok(Some(response.default_generation_settings.n_ctx))
    }

    async fn chat_stream(
        &self,
        model: &str,
        messages: &[Value],
        tools: &[Value],
        tool_choice: Option<Value>,
        thinking: ThinkingMode,
        max_completion_tokens: Option<u32>,
        sink: Option<&dyn TextSink>,
    ) -> anyhow::Result<ChatCompletion> {
        let ceiling = thinking.max_completion_tokens();
        let max_tokens = max_completion_tokens.map_or(ceiling, |cap| cap.min(ceiling));
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "max_tokens": max_tokens,
            "reasoning": thinking.reasoning_field(),
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
            body["tool_choice"] = tool_choice.unwrap_or_else(|| Value::String("auto".into()));
        }
        tracing::debug!(
            target: "housebot::llm",
            model,
            messages = messages.len(),
            tools = tools.len(),
            thinking = %thinking,
            "Starting streamed completion"
        );
        let started = std::time::Instant::now();

        let resp = self
            .http
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        let mut acc = Accumulator::default();
        let mut buf = String::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            buf.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(nl) = buf.find('\n') {
                let line: String = buf.drain(..=nl).collect();
                let line = line.trim_end();
                if line.is_empty() {
                    continue;
                }
                if let Some(parsed) = parse_sse_line(line) {
                    if let Some(delta) = acc.apply(parsed) {
                        if let Some(s) = sink {
                            let _ = delta;
                            s.push(&acc.content).await;
                        }
                    }
                }
            }
        }
        let completion = acc.finish();
        tracing::debug!(
            target: "housebot::llm",
            model,
            finish_reason = completion.finish_reason.as_deref().unwrap_or("none"),
            tool_calls = completion.tool_calls.len(),
            prompt_tokens = completion.usage.prompt_tokens,
            completion_tokens = completion.usage.completion_tokens,
            cached_tokens = completion.usage.prompt_tokens_details.cached_tokens,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Streamed completion finished"
        );
        Ok(completion)
    }

    async fn chat_once(
        &self,
        model: &str,
        messages: &[Value],
        max_tokens: u32,
    ) -> anyhow::Result<ChatCompletion> {
        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "max_tokens": max_tokens,
        });
        let resp = self
            .http
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json::<OnceResponse>()
            .await?;
        let content = resp
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();
        Ok(ChatCompletion {
            content: Some(content),
            finish_reason: Some("stop".into()),
            usage: resp.usage,
            ..Default::default()
        })
    }
}
