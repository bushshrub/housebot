//! Discord progress hooks and compaction progress rendering.

use std::sync::Mutex;

use super::*;

pub(crate) fn compact_progress(stage: usize, detail: Option<&str>) -> String {
    let filled = (stage / 10).min(10);
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled));
    match detail {
        Some(detail) => format!("🧠 **Compacting conversation**\n`[{bar}] {stage}%` — {detail}"),
        None => format!("🧠 **Compacting conversation**\n`[{bar}] {stage}%`"),
    }
}

pub(crate) enum CompactProgressTarget {
    Message {
        ctx: Context,
        channel_id: serenity::all::ChannelId,
        message_id: serenity::all::MessageId,
    },
    Interaction {
        ctx: Context,
        command: Box<serenity::all::CommandInteraction>,
    },
}

pub(crate) struct CompactProgressHooks(pub(crate) CompactProgressTarget);

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
        match &self.0 {
            CompactProgressTarget::Message {
                ctx,
                channel_id,
                message_id,
            } => {
                let _ = channel_id
                    .edit_message(&ctx.http, *message_id, EditMessage::new().content(content))
                    .await;
            }
            CompactProgressTarget::Interaction { ctx, command } => {
                let _ = command
                    .edit_response(&ctx.http, EditInteractionResponse::new().content(content))
                    .await;
            }
        }
    }
}

pub(crate) struct ResponseProgressHooks {
    ctx: Context,
    channel_id: serenity::all::ChannelId,
    message_id: serenity::all::MessageId,
    generating: AtomicBool,
    tool_calls: Mutex<String>,
}

impl ResponseProgressHooks {
    pub(crate) fn new(ctx: &Context, progress: &Message) -> Self {
        Self {
            ctx: ctx.clone(),
            channel_id: progress.channel_id,
            message_id: progress.id,
            generating: AtomicBool::new(false),
            tool_calls: Mutex::new(String::new()),
        }
    }
}

#[async_trait]
impl AgentHooks for ResponseProgressHooks {
    async fn on_text_stream(&self, _partial: &str) {
        if self.generating.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self
            .channel_id
            .edit_message(
                &self.ctx.http,
                self.message_id,
                EditMessage::new().content("⚙️ **Generating...**"),
            )
            .await;
    }

    async fn on_tool_called(&self, tool: &str, _args: &serde_json::Value) {
        self.generating.store(false, Ordering::Release);
        let content = {
            let mut calls = self.tool_calls.lock().unwrap();
            if !calls.is_empty() {
                calls.push('\n');
            }
            calls.push_str(tool_status(tool));
            calls.clone()
        };
        let _ = self
            .channel_id
            .edit_message(
                &self.ctx.http,
                self.message_id,
                EditMessage::new().content(content),
            )
            .await;
    }
}
