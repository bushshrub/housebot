//! Minimal client for an OpenAI-compatible chat-completions endpoint (llama.cpp).
//!
//! The [`ChatClient`] trait abstracts the LLM so the agent loop can be exercised in
//! tests with a scripted fake; [`OpenAiClient`] is the real streaming implementation.

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How much reasoning ("thinking") effort the model spends before answering.
///
/// Selected per user with the `/effort` slash command and sent to the
/// OpenAI-compatible backend as `reasoning_effort`. The levels are the ones the
/// deployed model accepts; the older levels are still read from stored configs
/// and mapped onto the nearest one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingMode {
    #[serde(alias = "instant")]
    Low,
    #[default]
    Medium,
    #[serde(alias = "high", alias = "max")]
    XHigh,
}

impl ThinkingMode {
    pub const ALL: [ThinkingMode; 3] =
        [ThinkingMode::Low, ThinkingMode::Medium, ThinkingMode::XHigh];

    /// Reserved for the visible answer, on top of the thinking allowance.
    const RESPONSE_TOKENS: u32 = 4096;

    /// Tokens the model may spend thinking before the request ceiling cuts it off.
    fn thinking_tokens(self) -> u32 {
        match self {
            ThinkingMode::Low => 2_048,
            ThinkingMode::Medium => 4_096,
            ThinkingMode::XHigh => 16_384,
        }
    }

    /// Human-readable description for command replies.
    pub fn description(self) -> &'static str {
        match self {
            ThinkingMode::Low => "quick answers, little thinking",
            ThinkingMode::Medium => "balanced",
            ThinkingMode::XHigh => "thinks longest",
        }
    }

    /// `max_tokens` for a completion request: thinking allowance plus room for the answer.
    pub fn max_completion_tokens(self) -> u32 {
        self.max_completion_tokens_capped(Self::RESPONSE_TOKENS)
    }

    /// `max_tokens` for a request whose visible answer is capped at
    /// `response_cap` tokens (a per-user output limit). The cap applies on top
    /// of the thinking allowance rather than to the request total: taking it
    /// out of the allowance would let reasoning consume the whole budget and
    /// leave the model no tokens to answer with.
    pub fn max_completion_tokens_capped(self, response_cap: u32) -> u32 {
        self.thinking_tokens() + response_cap.min(Self::RESPONSE_TOKENS)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ThinkingMode::Low => "low",
            ThinkingMode::Medium => "medium",
            ThinkingMode::XHigh => "xhigh",
        }
    }
}

impl std::str::FromStr for ThinkingMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ThinkingMode::ALL
            .into_iter()
            .find(|mode| mode.as_str() == s.to_ascii_lowercase())
            .ok_or(())
    }
}

impl std::fmt::Display for ThinkingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

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
    /// Query the server's configured per-sequence context window, when supported.
    async fn context_window_tokens(&self) -> anyhow::Result<Option<u64>>;

    /// Stream a completion, forwarding each cumulative text snapshot to `sink`.
    /// `thinking` sets the reasoning effort and the overall token ceiling;
    /// `max_completion_tokens` caps the visible answer when set (per-user
    /// output caps), leaving that budget intact. `tool_choice` overrides the
    /// default `"auto"` tool selection; pass `Some(json!("required"))` to force a tool call or
    /// `Some(json!({"type":"function","function":{"name":"…"}}))` to force a
    /// specific function. `None` keeps the default `"auto"` behavior.
    #[allow(clippy::too_many_arguments)]
    async fn chat_stream(
        &self,
        model: &str,
        messages: &[Value],
        tools: &[Value],
        tool_choice: Option<Value>,
        thinking: ThinkingMode,
        max_completion_tokens: Option<u32>,
        sink: Option<&dyn TextSink>,
    ) -> anyhow::Result<ChatCompletion>;

    /// Run a non-streaming completion and return the assistant's text.
    async fn chat_once(
        &self,
        model: &str,
        messages: &[Value],
        max_tokens: u32,
    ) -> anyhow::Result<ChatCompletion>;
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

    fn props_endpoint(&self) -> String {
        let root = self.base_url.strip_suffix("/v1").unwrap_or(&self.base_url);
        format!("{root}/props")
    }

    async fn post(&self, body: &Value) -> reqwest::Result<reqwest::Response> {
        self.http
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(body)
            .send()
            .await
    }

    /// POST a completion request. The gateway can route to a model that
    /// rejects `reasoning_effort` or the chosen level; that request is retried
    /// once with a thinking-token limit instead, rather than failing the
    /// user's turn. Models that ignore unknown fields never reach the retry.
    async fn post_with_effort_fallback(
        &self,
        body: Value,
        thinking: ThinkingMode,
    ) -> anyhow::Result<reqwest::Response> {
        let resp = self.post(&body).await?;
        if resp.status() != reqwest::StatusCode::BAD_REQUEST {
            return Ok(resp.error_for_status()?);
        }
        let error = resp.text().await.unwrap_or_default();
        if !rejects_reasoning_effort(&error) {
            anyhow::bail!("HTTP 400 Bad Request: {error}");
        }
        tracing::warn!(
            target: "housebot::llm",
            %error,
            "Model rejected reasoning_effort; retrying with a thinking-token limit"
        );
        let body = thinking_budget_body(body, thinking);
        Ok(self.post(&body).await?.error_for_status()?)
    }
}

/// `body` with `reasoning_effort` swapped for an OpenRouter-style
/// `reasoning.max_tokens` limit matching the same effort level.
fn thinking_budget_body(mut body: Value, thinking: ThinkingMode) -> Value {
    if let Some(fields) = body.as_object_mut() {
        fields.remove("reasoning_effort");
        fields.insert(
            "reasoning".into(),
            serde_json::json!({"enabled": true, "max_tokens": thinking.thinking_tokens()}),
        );
    }
    body
}

/// Whether a 400 response body is about the reasoning effort field.
fn rejects_reasoning_effort(error: &str) -> bool {
    error.to_ascii_lowercase().contains("reasoning")
}

#[derive(Deserialize)]
struct PropsResponse {
    default_generation_settings: DefaultGenerationSettings,
}

#[derive(Deserialize)]
struct DefaultGenerationSettings {
    n_ctx: u64,
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
    #[serde(default)]
    usage: TokenUsage,
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
        let max_tokens = max_completion_tokens.map_or_else(
            || thinking.max_completion_tokens(),
            |cap| thinking.max_completion_tokens_capped(cap),
        );
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "max_tokens": max_tokens,
            "reasoning_effort": thinking.as_str(),
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

        let resp = self.post_with_effort_fallback(body, thinking).await?;

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
    fn output_cap_limits_the_answer_without_eating_the_thinking_allowance() {
        // A 512-token user cap must still leave the thinking allowance intact,
        // otherwise thinking consumes the request and the answer comes back
        // empty.
        assert_eq!(
            ThinkingMode::Medium.max_completion_tokens_capped(512),
            4_096 + 512
        );
        // Caps above the reserved answer room leave the ceiling unchanged.
        assert_eq!(
            ThinkingMode::XHigh.max_completion_tokens_capped(100_000),
            ThinkingMode::XHigh.max_completion_tokens()
        );
    }

    #[test]
    fn thinking_mode_parses_and_displays() {
        for mode in ThinkingMode::ALL {
            assert_eq!(mode.as_str().parse::<ThinkingMode>(), Ok(mode));
        }
        assert_eq!("XHIGH".parse::<ThinkingMode>(), Ok(ThinkingMode::XHigh));
        assert!("high".parse::<ThinkingMode>().is_err());
    }

    #[test]
    fn stored_retired_levels_map_to_the_nearest_supported_one() {
        for (stored, mode) in [
            ("instant", ThinkingMode::Low),
            ("high", ThinkingMode::XHigh),
            ("max", ThinkingMode::XHigh),
        ] {
            assert_eq!(
                serde_json::from_str::<ThinkingMode>(&format!("\"{stored}\"")).unwrap(),
                mode
            );
        }
        assert_eq!(
            serde_json::to_string(&ThinkingMode::XHigh).unwrap(),
            "\"xhigh\""
        );
    }

    #[test]
    fn a_rejected_effort_is_retried_as_a_thinking_token_limit() {
        assert!(rejects_reasoning_effort(
            r#"{"error":{"message":"Unexpected reasoning effort high. Supported types are xhigh (default), medium, and low."}}"#
        ));
        assert!(!rejects_reasoning_effort(
            r#"{"error":{"message":"model not found"}}"#
        ));

        let body = serde_json::json!({"model": "m", "reasoning_effort": "low"});
        assert_eq!(
            thinking_budget_body(body, ThinkingMode::Low),
            serde_json::json!({
                "model": "m",
                "reasoning": {"enabled": true, "max_tokens": 2048}
            })
        );
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
