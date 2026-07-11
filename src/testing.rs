//! Test support utilities shared across module unit tests.
//!
//! These are intentionally part of the public crate surface so unit tests in any
//! module can drive the agent and tools without a live LLM.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;

use crate::llm::{ChatClient, ChatCompletion, TextSink, TokenUsage, ToolCall};

/// A scriptable, recording [`ChatClient`] for tests.
#[derive(Default)]
pub struct MockChatClient {
    stream_script: Mutex<VecDeque<ChatCompletion>>,
    once_reply: Mutex<String>,
    once_usage: Mutex<TokenUsage>,
    /// Messages passed to each `chat_stream` call, in order.
    pub stream_calls: Mutex<Vec<Vec<Value>>>,
    /// Messages passed to each `chat_once` call, in order.
    pub once_calls: Mutex<Vec<Vec<Value>>>,
}

impl MockChatClient {
    /// Create an empty mock (stream defaults to an empty "stop" completion).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the canned reply returned by [`ChatClient::chat_once`].
    pub fn with_once_reply(self, reply: &str) -> Self {
        *self.once_reply.lock().unwrap() = reply.to_string();
        self
    }

    pub fn with_once_usage(self, usage: TokenUsage) -> Self {
        *self.once_usage.lock().unwrap() = usage;
        self
    }

    /// Queue a streamed completion that emits `text` and finishes with `stop`.
    pub fn push_text(&self, text: &str) {
        self.push_completion(ChatCompletion {
            content: Some(text.to_string()),
            tool_calls: vec![],
            finish_reason: Some("stop".into()),
            usage: Default::default(),
        });
    }

    pub fn push_text_with_usage(&self, text: &str, usage: TokenUsage) {
        self.push_completion(ChatCompletion {
            content: Some(text.to_string()),
            tool_calls: vec![],
            finish_reason: Some("stop".into()),
            usage,
        });
    }

    fn push_completion(&self, completion: ChatCompletion) {
        self.stream_script.lock().unwrap().push_back(completion);
    }

    /// Queue a streamed completion requesting a single tool call.
    pub fn push_tool_call(&self, id: &str, name: &str, arguments: &str) {
        self.stream_script
            .lock()
            .unwrap()
            .push_back(ChatCompletion {
                content: None,
                tool_calls: vec![ToolCall {
                    id: id.to_string(),
                    name: name.to_string(),
                    arguments: arguments.to_string(),
                }],
                finish_reason: Some("tool_calls".into()),
                usage: Default::default(),
            });
    }
}

#[async_trait]
impl ChatClient for MockChatClient {
    async fn context_window_tokens(&self) -> anyhow::Result<Option<u64>> {
        Ok(Some(10_000))
    }

    async fn chat_stream(
        &self,
        _model: &str,
        messages: &[Value],
        _tools: &[Value],
        _max_tokens: u32,
        sink: Option<&dyn TextSink>,
    ) -> anyhow::Result<ChatCompletion> {
        self.stream_calls.lock().unwrap().push(messages.to_vec());
        let completion = self
            .stream_script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ChatCompletion {
                content: Some(String::new()),
                tool_calls: vec![],
                finish_reason: Some("stop".into()),
                usage: Default::default(),
            });
        if let (Some(sink), Some(text)) = (sink, completion.content.as_deref()) {
            sink.push(text).await;
        }
        Ok(completion)
    }

    async fn chat_once(
        &self,
        _model: &str,
        messages: &[Value],
        _max_tokens: u32,
    ) -> anyhow::Result<ChatCompletion> {
        self.once_calls.lock().unwrap().push(messages.to_vec());
        Ok(ChatCompletion {
            content: Some(self.once_reply.lock().unwrap().clone()),
            finish_reason: Some("stop".into()),
            usage: *self.once_usage.lock().unwrap(),
            ..Default::default()
        })
    }
}

/// A [`TextSink`] that records every pushed snapshot.
#[derive(Default)]
pub struct RecordingSink {
    pub pushes: Mutex<Vec<String>>,
}

#[async_trait]
impl TextSink for RecordingSink {
    async fn push(&self, partial: &str) {
        self.pushes.lock().unwrap().push(partial.to_string());
    }
}
