//! Discord progress hooks and compaction progress rendering.

use std::sync::Mutex;

use super::*;

const DISCORD_CONTENT_LIMIT: usize = 2000;

pub(crate) fn compact_progress(stage: usize, detail: Option<&str>) -> String {
    let filled = (stage / 10).min(10);
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled));
    match detail {
        Some(detail) => format!("🧠 **Compacting conversation**\n`[{bar}] {stage}%` — {detail}"),
        None => format!("🧠 **Compacting conversation**\n`[{bar}] {stage}%`"),
    }
}

pub(crate) struct CompactProgressHooks {
    ctx: Context,
    command: Box<serenity::all::CommandInteraction>,
}

impl CompactProgressHooks {
    pub(crate) fn new(ctx: Context, command: Box<serenity::all::CommandInteraction>) -> Self {
        Self { ctx, command }
    }
}

#[async_trait]
impl AgentHooks for CompactProgressHooks {
    async fn on_progress(&self, line: &str) {
        let Some(rest) = line.strip_prefix("compact:") else {
            return;
        };
        let (stage, detail) = rest.split_once(':').unwrap_or((rest, ""));
        let Ok(stage) = stage.parse::<usize>() else {
            return;
        };
        let content = compact_progress(stage, (!detail.is_empty()).then_some(detail));
        let _ = self
            .command
            .edit_response(
                &self.ctx.http,
                EditInteractionResponse::new().content(content),
            )
            .await;
    }
}

/// Keeps Discord's "is typing…" indicator alive for as long as it is held.
///
/// Discord expires the indicator after ~10s, so it is refreshed on a timer and
/// dropped once the turn is over — including the stretches where nothing is
/// streaming, such as queue waits and tool execution.
pub(crate) struct TypingIndicator(tokio::task::JoinHandle<()>);

impl TypingIndicator {
    pub(crate) fn start(ctx: &Context, channel_id: serenity::all::ChannelId) -> Self {
        let http = ctx.http.clone();
        Self(tokio::spawn(async move {
            loop {
                let _ = channel_id.broadcast_typing(&http).await;
                tokio::time::sleep(Duration::from_secs(8)).await;
            }
        }))
    }
}

impl Drop for TypingIndicator {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) struct ResponseProgressHooks {
    ctx: Context,
    channel_id: serenity::all::ChannelId,
    message_id: serenity::all::MessageId,
    generating: AtomicBool,
    tool_calls: Mutex<String>,
    redactor: Arc<SecretRedactor>,
}

impl ResponseProgressHooks {
    pub(crate) fn new(ctx: &Context, progress: &Message, redactor: Arc<SecretRedactor>) -> Self {
        Self {
            ctx: ctx.clone(),
            channel_id: progress.channel_id,
            message_id: progress.id,
            generating: AtomicBool::new(false),
            tool_calls: Mutex::new(String::new()),
            redactor,
        }
    }
}

#[async_trait]
impl AgentHooks for ResponseProgressHooks {
    async fn on_text_stream(&self, _partial: &str) {
        if self.generating.swap(true, Ordering::AcqRel) {
            return;
        }
        let content = {
            let calls = self.tool_calls.lock().unwrap();
            if calls.is_empty() {
                "⚙️ **Generating...**".to_string()
            } else {
                format!("{calls}\n⚙️ **Generating...**")
            }
        };
        if let Err(e) = self
            .channel_id
            .edit_message(
                &self.ctx.http,
                self.message_id,
                EditMessage::new().content(content),
            )
            .await
        {
            tracing::warn!(%e, "Failed to update text-stream progress message");
        }
    }

    async fn on_tool_called(&self, tool: &str, args: &serde_json::Value) {
        self.generating.store(false, Ordering::Release);
        let content = {
            let mut calls = self.tool_calls.lock().unwrap();
            if !calls.is_empty() {
                calls.push('\n');
            }
            let status = tool_status(tool);
            let hint = tool_hint(tool, args);
            if hint.is_empty() {
                calls.push_str(&status);
            } else {
                let base = status.strip_suffix("...**").unwrap_or(&status);
                calls.push_str(&format!("{base}{hint}...**"));
            }
            while calls.chars().count() > DISCORD_CONTENT_LIMIT {
                if let Some(pos) = calls.find('\n') {
                    calls.drain(..pos + 1);
                } else {
                    break;
                }
            }
            let redacted = self.redactor.redact(&calls);
            *calls = redacted.clone();
            redacted
        };
        if let Err(e) = self
            .channel_id
            .edit_message(
                &self.ctx.http,
                self.message_id,
                EditMessage::new().content(content),
            )
            .await
        {
            tracing::warn!(%e, "Failed to update tool-call progress message");
        }
    }
}
