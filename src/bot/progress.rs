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

pub(crate) struct ResponseProgressHooks {
    ctx: Context,
    channel_id: serenity::all::ChannelId,
    message_id: serenity::all::MessageId,
    generating: AtomicBool,
    typing_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    tool_calls: Mutex<String>,
}

impl ResponseProgressHooks {
    pub(crate) fn new(ctx: &Context, progress: &Message) -> Self {
        Self {
            ctx: ctx.clone(),
            channel_id: progress.channel_id,
            message_id: progress.id,
            generating: AtomicBool::new(false),
            typing_task: Mutex::new(None),
            tool_calls: Mutex::new(String::new()),
        }
    }

    fn start_typing(&self) {
        let mut task = self.typing_task.lock().unwrap();
        if task.is_some() {
            return;
        }
        let http = self.ctx.http.clone();
        let channel_id = self.channel_id;
        *task = Some(tokio::spawn(async move {
            loop {
                let _ = channel_id.broadcast_typing(&http).await;
                tokio::time::sleep(Duration::from_secs(8)).await;
            }
        }));
    }

    fn stop_typing(&self) {
        if let Some(task) = self.typing_task.lock().unwrap().take() {
            task.abort();
        }
    }
}

impl Drop for ResponseProgressHooks {
    fn drop(&mut self) {
        if let Some(task) = self.typing_task.get_mut().unwrap().take() {
            task.abort();
        }
    }
}

#[async_trait]
impl AgentHooks for ResponseProgressHooks {
    async fn on_text_stream(&self, _partial: &str) {
        self.start_typing();
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

    async fn on_text_stream_end(&self) {
        self.stop_typing();
    }

    async fn on_tool_called(&self, tool: &str, _args: &serde_json::Value) {
        self.stop_typing();
        self.generating.store(false, Ordering::Release);
        let content = {
            let mut calls = self.tool_calls.lock().unwrap();
            if !calls.is_empty() {
                calls.push('\n');
            }
            calls.push_str(&tool_status(tool));
            while calls.chars().count() > DISCORD_CONTENT_LIMIT {
                if let Some(pos) = calls.find('\n') {
                    calls.drain(..pos + 1);
                } else {
                    break;
                }
            }
            calls.clone()
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
