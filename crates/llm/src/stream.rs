//! Stream.

use crate::*;
use serde::Deserialize;

// ── streaming wire format ────────────────────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct StreamChunk {
    #[serde(default)]
    pub(crate) choices: Vec<StreamChoice>,
    #[serde(default)]
    pub(crate) usage: Option<TokenUsage>,
}

#[derive(Deserialize, Default)]
pub(crate) struct StreamChoice {
    #[serde(default)]
    pub(crate) delta: Delta,
    #[serde(default)]
    pub(crate) finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
pub(crate) struct Delta {
    #[serde(default)]
    pub(crate) content: Option<String>,
    #[serde(default)]
    pub(crate) tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Deserialize)]
pub(crate) struct ToolCallDelta {
    pub(crate) index: usize,
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(default)]
    pub(crate) function: Option<FnDelta>,
}

#[derive(Deserialize, Default)]
pub(crate) struct FnDelta {
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) arguments: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct OnceResponse {
    pub(crate) choices: Vec<OnceChoice>,
    #[serde(default)]
    pub(crate) usage: TokenUsage,
}

#[derive(Deserialize)]
pub(crate) struct OnceChoice {
    pub(crate) message: OnceMessage,
}

#[derive(Deserialize)]
pub(crate) struct OnceMessage {
    #[serde(default)]
    pub(crate) content: Option<String>,
}

/// Accumulates streamed deltas into a [`ChatCompletion`].
#[derive(Default)]
pub(crate) struct Accumulator {
    pub(crate) content: String,
    pub(crate) tool_calls: Vec<(String, String, String)>, // (id, name, arguments) indexed by slot
    pub(crate) finish_reason: Option<String>,
    pub(crate) usage: TokenUsage,
}

impl Accumulator {
    /// Apply one decoded chunk, returning the new content delta (if any) for streaming.
    pub(crate) fn apply(&mut self, chunk: StreamChunk) -> Option<String> {
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
                const MAX_TOOL_CALL_SLOTS: usize = 256;
                if tc.index >= MAX_TOOL_CALL_SLOTS {
                    continue;
                }
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

    pub(crate) fn finish(self) -> ChatCompletion {
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
pub(crate) fn parse_sse_line(line: &str) -> Option<StreamChunk> {
    let payload = line.strip_prefix("data:")?.trim();
    if payload.is_empty() || payload == "[DONE]" {
        return None;
    }
    serde_json::from_str(payload).ok()
}
