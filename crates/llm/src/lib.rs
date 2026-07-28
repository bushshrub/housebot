//! Minimal client for an OpenAI-compatible chat-completions endpoint (llama.cpp).
//!
//! The [`ChatClient`] trait abstracts the LLM so the agent loop can be exercised in
//! tests with a scripted fake; [`OpenAiClient`] is the real streaming implementation.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod client;
pub mod stream;

pub use client::*;

/// How much reasoning ("thinking") budget the model gets before answering.
///
/// Selected per user with the `/effort` slash command and forwarded to the
/// OpenAI-compatible backend as a `reasoning` request field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingMode {
    Instant,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    Max,
}

impl ThinkingMode {
    pub const ALL: [ThinkingMode; 6] = [
        ThinkingMode::Instant,
        ThinkingMode::Low,
        ThinkingMode::Medium,
        ThinkingMode::High,
        ThinkingMode::XHigh,
        ThinkingMode::Max,
    ];

    /// Reserved for the visible answer, on top of any thinking budget.
    const RESPONSE_TOKENS: u32 = 4096;

    /// Thinking-token budget; `None` means unlimited.
    pub fn budget_tokens(self) -> Option<u32> {
        match self {
            ThinkingMode::Instant => Some(0),
            ThinkingMode::Low => Some(2_048),
            ThinkingMode::Medium => Some(4_096),
            ThinkingMode::High => Some(8_192),
            ThinkingMode::XHigh => Some(16_384),
            ThinkingMode::Max => None,
        }
    }

    /// Human-readable budget for command replies.
    pub fn budget_label(self) -> &'static str {
        match self {
            ThinkingMode::Instant => "no thinking",
            ThinkingMode::Low => "2k thinking tokens",
            ThinkingMode::Medium => "4k thinking tokens",
            ThinkingMode::High => "8k thinking tokens",
            ThinkingMode::XHigh => "16k thinking tokens",
            ThinkingMode::Max => "unlimited thinking tokens",
        }
    }

    /// `max_tokens` for a completion request: thinking budget plus room for the answer.
    pub fn max_completion_tokens(self) -> u32 {
        match self.budget_tokens() {
            Some(budget) => budget + Self::RESPONSE_TOKENS,
            None => 32_768,
        }
    }

    /// The `reasoning` request field sent to the backend (OpenRouter-style;
    /// servers that don't support it ignore unknown fields).
    pub fn reasoning_field(self) -> Value {
        if self == ThinkingMode::Instant {
            return serde_json::json!({"enabled": false});
        }
        match self.budget_tokens() {
            Some(budget) => serde_json::json!({"enabled": true, "max_tokens": budget}),
            None => serde_json::json!({"enabled": true}),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ThinkingMode::Instant => "instant",
            ThinkingMode::Low => "low",
            ThinkingMode::Medium => "medium",
            ThinkingMode::High => "high",
            ThinkingMode::XHigh => "xhigh",
            ThinkingMode::Max => "max",
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
    /// `thinking` sets the reasoning budget and the overall token ceiling;
    /// `max_completion_tokens` further lowers that ceiling when set (per-user
    /// output caps). `tool_choice` overrides the default `"auto"` tool
    /// selection; pass `Some(json!("required"))` to force a tool call or
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

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
