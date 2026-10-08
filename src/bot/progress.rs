//! Discord progress hooks and compaction progress rendering.

use super::*;

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

const THINKING: &str = "🧠 **Thinking...**";
const GENERATING: &str = "⚙️ **Generating...**";
/// Discord's limit on a thread name.
const THREAD_NAME_LIMIT: usize = 100;

/// Shows a turn's progress in Discord. The progress message carries one
/// status line; tool calls and the text written between them go to a thread
/// opened from that message on first use, so the channel itself only gets the
/// status line and the final reply.
pub(crate) struct ResponseProgressHooks {
    ctx: Context,
    channel_id: serenity::all::ChannelId,
    message_id: serenity::all::MessageId,
    thread_name: String,
    state: Mutex<ProgressState>,
    redactor: Arc<SecretRedactor>,
}

#[derive(Default)]
struct ProgressState {
    /// The status line last written, so a stream of deltas edits only on change.
    shown: String,
    thread: WorkThread,
}

#[derive(Default, Clone, Copy)]
enum WorkThread {
    #[default]
    NotOpened,
    Open(serenity::all::ChannelId),
    /// No thread can be made here: a DM, a thread already, or no permission.
    Unavailable,
}

/// A thread name taken from the user's request: its first line, cut to fit.
pub(crate) fn work_thread_name(request: &str) -> String {
    let first_line = request.lines().find(|line| !line.trim().is_empty());
    let title = first_line.unwrap_or("Tool calls").trim();
    let limit = THREAD_NAME_LIMIT - 4;
    let mut name: String = format!("🛠️ {title}").chars().take(limit).collect();
    if title.chars().count() + 3 > limit {
        name.push('…');
    }
    name
}

impl ResponseProgressHooks {
    pub(crate) fn new(
        ctx: &Context,
        progress: &Message,
        thread_name: String,
        redactor: Arc<SecretRedactor>,
    ) -> Self {
        Self {
            ctx: ctx.clone(),
            channel_id: progress.channel_id,
            message_id: progress.id,
            thread_name,
            state: Mutex::new(ProgressState {
                shown: progress.content.clone(),
                thread: WorkThread::NotOpened,
            }),
            redactor,
        }
    }

    async fn set_status(&self, state: &mut ProgressState, status: String) {
        if state.shown == status {
            return;
        }
        let edit = EditMessage::new().content(&status);
        if let Err(error) = self
            .channel_id
            .edit_message(&self.ctx.http, self.message_id, edit)
            .await
        {
            tracing::warn!(%error, "Failed to update the progress message");
        }
        state.shown = status;
    }

    /// The thread for this turn's work, opened on first use.
    async fn work_thread(&self, state: &mut ProgressState) -> Option<serenity::all::ChannelId> {
        if let WorkThread::NotOpened = state.thread {
            let builder = serenity::all::CreateThread::new(&self.thread_name)
                .auto_archive_duration(serenity::all::AutoArchiveDuration::OneHour);
            state.thread = match self
                .channel_id
                .create_thread_from_message(&self.ctx.http, self.message_id, builder)
                .await
            {
                Ok(thread) => WorkThread::Open(thread.id),
                Err(error) => {
                    tracing::debug!(%error, "No work thread here; tool calls stay in the status line");
                    WorkThread::Unavailable
                }
            };
        }
        match state.thread {
            WorkThread::Open(id) => Some(id),
            _ => None,
        }
    }

    async fn post(&self, channel_id: serenity::all::ChannelId, text: &str) {
        for chunk in split_text(text, MAX_MESSAGE_LENGTH) {
            let message = CreateMessage::new()
                .content(chunk)
                .allowed_mentions(CreateAllowedMentions::new());
            if let Err(error) = channel_id.send_message(&self.ctx.http, message).await {
                tracing::warn!(%error, "Failed to post to the work thread");
            }
        }
    }

    /// End the turn: with tool calls, the status line becomes their summary and
    /// the thread is archived; without, the progress message is deleted.
    pub(crate) async fn finish(&self, tools: &[String]) {
        let mut state = self.state.lock().await;
        let thread = match state.thread {
            WorkThread::Open(id) => Some(id),
            _ => None,
        };
        if tools.is_empty() && thread.is_none() {
            let _ = self
                .channel_id
                .delete_message(&self.ctx.http, self.message_id)
                .await;
            return;
        }
        let mut summary = tool_summary(tools);
        if let Some(id) = thread {
            summary.push_str(&format!(" · <#{id}>"));
        }
        self.set_status(&mut state, summary).await;
        if let Some(id) = thread {
            let archive = serenity::all::EditThread::new().archived(true);
            if let Err(error) = id.edit_thread(&self.ctx.http, archive).await {
                tracing::debug!(%error, "Failed to archive the work thread");
            }
        }
    }
}

#[async_trait]
impl AgentHooks for ResponseProgressHooks {
    /// An empty partial marks the start of a model round, before any text:
    /// the model is still reasoning.
    async fn on_text_stream(&self, partial: &str) {
        let mut state = self.state.lock().await;
        let status = if partial.is_empty() {
            THINKING
        } else {
            GENERATING
        };
        self.set_status(&mut state, status.to_string()).await;
    }

    async fn on_assistant_text(&self, text: &str) {
        let mut state = self.state.lock().await;
        let redacted = self.redactor.redact(text);
        let target = self
            .work_thread(&mut state)
            .await
            .unwrap_or(self.channel_id);
        self.post(target, &redacted).await;
    }

    async fn on_tool_called(&self, tool: &str, args: &serde_json::Value) {
        let mut state = self.state.lock().await;
        let mut status = tool_status(tool);
        if let Some(thread) = self.work_thread(&mut state).await {
            let content = self.redactor.redact(&tool_message(tool, args));
            self.post(thread, &content).await;
            status.push_str(&format!("\n-# Details in <#{thread}>"));
        }
        self.set_status(&mut state, status).await;
    }
}
