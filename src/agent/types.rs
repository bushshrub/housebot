//! Data types exchanged with the agent: requests, results, hooks, outcomes.

use super::*;

/// An inbound media attachment, base64-encoded for the multimodal API.
#[derive(Debug, Clone)]
pub struct MediaData {
    pub media_type: String,
    pub data: String,
}

/// A one-shot cancellation flag for an active agent run.  When the flag is
/// triggered the agent loop stops as soon as possible.
#[derive(Debug, Default)]
struct CancelState {
    cancelled: AtomicBool,
    notify: Notify,
}

#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<CancelState>);

impl CancelToken {
    pub(crate) fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        self.0.notify.notify_waiters();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    pub(crate) async fn cancelled(&self) {
        loop {
            let notified = self.0.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// One user turn to run through the agent.
#[derive(Debug, Clone)]
pub struct AgentRequest<'a> {
    pub user_id: &'a str,
    pub username: &'a str,
    pub text: &'a str,
    pub media: &'a [MediaData],
    /// Optional personality/tone override injected into the system prompt.
    pub personality: Option<&'a str>,
    /// Reasoning budget for this user's requests.
    pub thinking: ThinkingMode,
    /// Discord channel ID (0 if unknown). Used by the `prepare_feature_development` tool.
    pub channel_id: u64,
    /// Whether deep memory (update_memory tool + auto-summary) is enabled for this user.
    pub deep_memory_enabled: bool,
    /// User's display name from their profile (for personalized greetings).
    pub display_name: &'a str,
    /// User's guild nickname from their profile (empty if none).
    pub nickname: &'a str,
    /// User's Discord avatar URL from their persisted profile (empty if none).
    pub avatar_url: &'a str,
    pub profile_tags: &'a str,
    pub quick_actions: &'a str,
    pub guild_id: Option<u64>,
    pub proactive: bool,
    pub record_profile_usage: bool,
    /// Per-user cap on completion output tokens, set by the bot's configurers.
    pub max_output_tokens: Option<u32>,
    /// Optional cancellation token. When triggered, the active LLM stream is
    /// dropped and the agent loop stops without producing a response.
    pub cancel: Option<CancelToken>,
}

impl<'a> AgentRequest<'a> {
    /// A plain text request with default settings (used by tests and headless callers).
    pub fn text(user_id: &'a str, username: &'a str, text: &'a str) -> Self {
        Self {
            user_id,
            username,
            text,
            media: &[],
            personality: None,
            thinking: ThinkingMode::default(),
            channel_id: 0,
            deep_memory_enabled: true,
            display_name: username,
            nickname: "",
            avatar_url: "",
            profile_tags: "",
            quick_actions: "",
            guild_id: None,
            proactive: false,
            record_profile_usage: true,
            max_output_tokens: None,
            cancel: None,
        }
    }
}

/// Structured bot-control action extracted from a tool call, carried alongside text.
#[derive(Debug, Clone)]
pub enum AgentControlAction {
    /// Owner wants to configure interactively.
    OwnerConfigurationRequired { job_id: uuid::Uuid },
    /// Non-owner request created; owner must approve.
    OwnerApprovalRequired { job_id: uuid::Uuid },
}

/// A file produced by an agent tool for direct delivery to Discord.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAttachment {
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// The outcome of one `Agent::run`.
#[derive(Debug, Clone, Default)]
pub struct AgentResult {
    pub text: String,
    pub session_notice: Option<String>,
    pub tools_called: Vec<String>,
    pub attachments: Vec<AgentAttachment>,
    /// Set when a `prepare_feature_development` tool call produces a structured outcome.
    pub control_action: Option<AgentControlAction>,
    /// Set when the user cancelled this request mid-generation.
    pub cancelled: bool,
}

/// The result of the pre-execution Lua safety review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaAnalysis {
    pub allowed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Copy)]
pub struct SessionInfo {
    pub context_tokens: usize,
    pub context_window_tokens: usize,
    pub messages: usize,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
}

/// Per-request callbacks used to surface progress into the chat surface.
#[async_trait]
pub trait AgentHooks: Send + Sync {
    /// Cumulative assistant text as it streams in.
    async fn on_text_stream(&self, _partial: &str) {}
    /// The current assistant text stream has ended.
    async fn on_text_stream_end(&self) {}
    /// A tool is about to run.
    async fn on_tool_called(&self, _tool: &str, _args: &Value) {}
    /// A progress update from a long-running operation.
    async fn on_progress(&self, _line: &str) {}
}

/// No-op hooks (used in tests and headless contexts).
pub struct NoHooks;
#[async_trait]
impl AgentHooks for NoHooks {}

pub(crate) struct TextStreamAdapter<'a>(pub(crate) &'a dyn AgentHooks);
#[async_trait]
impl TextSink for TextStreamAdapter<'_> {
    async fn push(&self, partial: &str) {
        self.0.on_text_stream(partial).await;
    }
}

/// Result of dispatching a single tool call.
#[derive(Debug)]
pub(crate) enum ToolOutcome {
    Text(String),
    Attachment {
        text: String,
        attachment: AgentAttachment,
    },
    /// A development-flow tool call that also carries a control action.
    DevelopmentAction {
        text: String,
        action: AgentControlAction,
    },
}
