//! Minimal client for an OpenAI-compatible chat-completions endpoint (llama.cpp).
//!
//! The [`ChatClient`] trait abstracts the LLM so the agent loop can be exercised in
//! tests with a scripted fake; [`OpenAiClient`] is the real streaming implementation.

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::Value;

/// A single tool call requested by the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// The assembled result of one chat completion.
#[derive(Debug, Clone, Default)]
pub struct ChatCompletion {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub prompt_tokens_details: PromptTokenDetails,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct PromptTokenDetails {
    #[serde(default)]
    pub cached_tokens: u64,
}

/// Async sink for incremental assistant text (used to stream into Discord).
#[async_trait]
pub trait TextSink: Send + Sync {
    async fn push(&self, partial: &str);
}

/// Abstraction over the chat-completions API.
#[async_trait]
pub trait ChatClient: Send + Sync {
    /// Stream a completion, forwarding each cumulative text snapshot to `sink`.
    async fn chat_stream(
        &self,
        model: &str,
        messages: &[Value],
        tools: &[Value],
        max_tokens: u32,
        sink: Option<&dyn TextSink>,
    ) -> anyhow::Result<ChatCompletion>;

    /// Run a non-streaming completion and return the assistant's text.
    async fn chat_once(
        &self,
        model: &str,
        messages: &[Value],
        max_tokens: u32,
    ) -> anyhow::Result<String>;
}

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
}

// ── streaming wire format ────────────────────────────────────────────────────

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<TokenUsage>,
}

#[derive(Deserialize, Default)]
struct StreamChoice {
    #[serde(default)]
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Deserialize)]
struct ToolCallDelta {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FnDelta>,
}

#[derive(Deserialize, Default)]
struct FnDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct OnceResponse {
    choices: Vec<OnceChoice>,
}

#[derive(Deserialize)]
struct OnceChoice {
    message: OnceMessage,
}

#[derive(Deserialize)]
struct OnceMessage {
    #[serde(default)]
    content: Option<String>,
}

/// Accumulates streamed deltas into a [`ChatCompletion`].
#[derive(Default)]
struct Accumulator {
    content: String,
    tool_calls: Vec<(String, String, String)>, // (id, name, arguments) indexed by slot
    finish_reason: Option<String>,
    usage: TokenUsage,
}

impl Accumulator {
    /// Apply one decoded chunk, returning the new content delta (if any) for streaming.
    fn apply(&mut self, chunk: StreamChunk) -> Option<String> {
        if let Some(usage) = chunk.usage {
            self.usage = usage;
        }
        let choice = chunk.choices.into_iter().next()?;
        if choice.finish_reason.is_some() {
            self.finish_reason = choice.finish_reason;
        }
        let mut new_text = None;
        if let Some(text) = choice.delta.content {
            if !text.is_empty() {
                self.content.push_str(&text);
                new_text = Some(text);
            }
        }
        if let Some(tcs) = choice.delta.tool_calls {
            for tc in tcs {
                while self.tool_calls.len() <= tc.index {
                    self.tool_calls
                        .push((String::new(), String::new(), String::new()));
                }
                let slot = &mut self.tool_calls[tc.index];
                if let Some(id) = tc.id {
                    slot.0 = id;
                }
                if let Some(f) = tc.function {
                    if let Some(name) = f.name {
                        slot.1.push_str(&name);
                    }
                    if let Some(args) = f.arguments {
                        slot.2.push_str(&args);
                    }
                }
            }
        }
        new_text
    }

    fn finish(self) -> ChatCompletion {
        ChatCompletion {
            content: if self.content.is_empty() {
                None
            } else {
                Some(self.content)
            },
            tool_calls: self
                .tool_calls
                .into_iter()
                .filter(|(_, name, _)| !name.is_empty())
                .map(|(id, name, arguments)| ToolCall {
                    id,
                    name,
                    arguments,
                })
                .collect(),
            finish_reason: self.finish_reason,
            usage: self.usage,
        }
    }
}

/// Parse one buffered SSE `data:` payload; `[DONE]` yields `None`.
fn parse_sse_line(line: &str) -> Option<StreamChunk> {
    let payload = line.strip_prefix("data:")?.trim();
    if payload.is_empty() || payload == "[DONE]" {
        return None;
    }
    serde_json::from_str(payload).ok()
}

#[async_trait]
impl ChatClient for OpenAiClient {
    async fn chat_stream(
        &self,
        model: &str,
        messages: &[Value],
        tools: &[Value],
        max_tokens: u32,
        sink: Option<&dyn TextSink>,
    ) -> anyhow::Result<ChatCompletion> {
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "max_tokens": max_tokens,
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
            body["tool_choice"] = Value::String("auto".into());
        }

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
        Ok(acc.finish())
    }

    async fn chat_once(
        &self,
        model: &str,
        messages: &[Value],
        max_tokens: u32,
    ) -> anyhow::Result<String> {
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
        Ok(resp
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(json: &str) -> StreamChunk {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn accumulator_collects_text() {
        let mut acc = Accumulator::default();
        acc.apply(chunk(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#));
        acc.apply(chunk(r#"{"choices":[{"delta":{"content":"lo"}}]}"#));
        acc.apply(chunk(
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        ));
        let done = acc.finish();
        assert_eq!(done.content.as_deref(), Some("Hello"));
        assert_eq!(done.finish_reason.as_deref(), Some("stop"));
        assert!(done.tool_calls.is_empty());
    }

    #[test]
    fn accumulator_assembles_tool_calls() {
        let mut acc = Accumulator::default();
        acc.apply(chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"trans","arguments":"{\"a\":"}}]}}]}"#,
        ));
        acc.apply(chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]},"finish_reason":"tool_calls"}]}"#,
        ));
        let done = acc.finish();
        assert_eq!(done.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(done.tool_calls.len(), 1);
        assert_eq!(done.tool_calls[0].id, "call_1");
        assert_eq!(done.tool_calls[0].name, "trans");
        assert_eq!(done.tool_calls[0].arguments, "{\"a\":1}");
    }

    #[test]
    fn apply_returns_content_delta() {
        let mut acc = Accumulator::default();
        let d = acc.apply(chunk(r#"{"choices":[{"delta":{"content":"hi"}}]}"#));
        assert_eq!(d.as_deref(), Some("hi"));
    }

    #[test]
    fn parse_sse_line_handles_done_and_data() {
        assert!(parse_sse_line("data: [DONE]").is_none());
        assert!(parse_sse_line(": comment").is_none());
        assert!(parse_sse_line(r#"data: {"choices":[]}"#).is_some());
    }

    #[test]
    fn empty_tool_calls_are_dropped() {
        // A slot that never received a name must not surface as a tool call.
        let mut acc = Accumulator::default();
        acc.apply(chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x"}]}}]}"#,
        ));
        assert!(acc.finish().tool_calls.is_empty());
    }
}
