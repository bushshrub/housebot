//! The agentic loop: builds prompts, streams completions from the LLM, dispatches tool
//! calls, and persists per-user history and memory.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Local, Utc};
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::bot_config::{
    AccessControl, AccessControlStore, ClassifierStore, SchedulerLimits, SchedulerLimitsStore,
};
use crate::channel_context::ChannelContext;
use crate::coding_agent::pending::PendingJobStore;
use crate::config;
use crate::discord_bridge::DiscordBridge;
use crate::github_issues::GitHubIssueReporter;
use crate::history::History;
use crate::llm::{ChatClient, OpenAiClient, TextSink, ThinkingMode, TokenUsage};
use crate::llm_scheduler::{LlmScheduler, ScheduledChatClient, SchedulerInfo};
use crate::memory::Memory;
use crate::rate_limit::RateLimiter;
use crate::reminders::Reminders;
use crate::skills::{Skill, Skills};
use crate::token_monitor::{
    LeaderboardEntry, LeaderboardMetric, LeaderboardPeriod, TokenLeaderboard, TokenMonitor,
};
use crate::tools;
use crate::tools::sandbox::LazySandbox;
use crate::tools::searxng::SearxNg;
use crate::tools::web_fetch::WebFetch;

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

/// How many tool calls in a row the model may make before a turn is stopped.
/// Every round is a full LLM round trip, so this keeps a runaway loop bounded.
pub const MAX_TOOL_ROUNDS: usize = 50;

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
    /// User's display name from their profile (for personalized greetings).
    pub display_name: &'a str,
    /// User's guild nickname from their profile (empty if none).
    pub nickname: &'a str,
    /// User's Discord avatar URL from their persisted profile (empty if none).
    pub avatar_url: &'a str,
    pub guild_id: Option<u64>,
    /// Per-user cap on completion output tokens, set by the bot's configurers.
    pub max_output_tokens: Option<u32>,
    /// How many tool calls in a row the model may make before the turn is stopped.
    pub max_tool_rounds: usize,
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
            display_name: username,
            nickname: "",
            avatar_url: "",
            guild_id: None,
            max_output_tokens: None,
            max_tool_rounds: MAX_TOOL_ROUNDS,
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

/// The outcome of one `Agent::run`.
#[derive(Debug, Clone, Default)]
pub struct AgentResult {
    pub text: String,
    pub session_notice: Option<String>,
    pub tools_called: Vec<String>,
    /// Set when a `prepare_feature_development` tool call produces a structured outcome.
    pub control_action: Option<AgentControlAction>,
    /// Set when the user cancelled this request mid-generation.
    pub cancelled: bool,
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
    /// Cumulative assistant text as it streams in. An empty string marks the
    /// start of a model round, before any text.
    async fn on_text_stream(&self, _partial: &str) {}
    /// The current assistant text stream has ended.
    async fn on_text_stream_end(&self) {}
    /// Text the model wrote alongside tool calls, before those tools run.
    async fn on_assistant_text(&self, _text: &str) {}
    /// A tool is about to run.
    async fn on_tool_called(&self, _tool: &str, _args: &Value) {}
    /// A progress update from a long-running operation.
    async fn on_progress(&self, _line: &str) {}
}

/// No-op hooks (used in tests and headless contexts).
pub struct NoHooks;
#[async_trait]
impl AgentHooks for NoHooks {}

struct TextStreamAdapter<'a>(&'a dyn AgentHooks);
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
    /// A development-flow tool call that also carries a control action.
    DevelopmentAction {
        text: String,
        action: AgentControlAction,
    },
}

/// The agent: LLM client, storage, and tools.
pub struct Agent {
    client: Arc<dyn ChatClient>,
    scheduled_client: Arc<ScheduledChatClient>,
    model: String,
    context_window_tokens: usize,
    history: History,
    memory: Memory,
    skills: Skills,
    reminders: Reminders,
    reporter: Arc<GitHubIssueReporter>,
    rate_limiter: RateLimiter,
    feature_edit_limiter: RateLimiter,
    /// Non-owner per-user development request limiter.
    non_owner_dev_limiter: RateLimiter,
    /// Owner safety limiter — consumed only at actual GitHub dispatch (reserved for future use).
    #[allow(dead_code)]
    owner_dispatch_limiter: RateLimiter,
    pending_jobs: Arc<PendingJobStore>,
    searxng: Arc<SearxNg>,
    web_fetch: WebFetch,
    session_stats: tokio::sync::Mutex<HashMap<String, SessionStats>>,
    token_monitor: TokenMonitor,
    active_conversations: tokio::sync::Mutex<HashMap<String, String>>,
    access_control: AccessControlStore,
    scheduler_limits: SchedulerLimitsStore,
    classifier_store: ClassifierStore,
    classifier: std::sync::RwLock<Option<Arc<classify::Classifier>>>,
    discord: Arc<DiscordBridge>,
    channel_context: ChannelContext,
    sandbox_client: housebot_sandbox::SandboxClient,
    /// Audit trail of administrator pull-request merges.
    merge_audit: tools::github_api::MergeAuditLog,
}

mod classify;
mod dispatch;
mod leaderboard_fmt;
mod prompt;
mod run;
mod session;
mod tools_def;

pub use classify::{classifier_state, ProactiveAction, CLASSIFIER_CONTEXT_MESSAGES};
#[allow(unused_imports)]
use leaderboard_fmt::*;
pub use prompt::build_system_prompt;
#[allow(unused_imports)]
use prompt::*;
#[allow(unused_imports)]
use tools_def::*;
pub use tools_def::{flatten_tool, to_openai_tool};

#[derive(Debug, Clone, Copy, Default)]
struct SessionStats {
    requests: u64,
    context_tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
}

impl Agent {
    /// Build an agent from environment configuration and start MCP servers.
    pub async fn from_env(discord: Arc<DiscordBridge>) -> anyhow::Result<Self> {
        let raw_client: Arc<dyn ChatClient> = Arc::new(OpenAiClient::new(
            config::env_or("LLM_BASE_URL", "http://server-slop:8080/v1"),
            config::env_or("LLM_API_KEY", "not-required"),
        ));
        let context_window_tokens = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            raw_client.context_window_tokens(),
        )
        .await
        .unwrap_or(Ok(None))
        .ok()
        .flatten()
        .map(|tokens| tokens as usize)
        .unwrap_or_else(|| {
            tracing::warn!(
                "LLM /props probe timed out or failed — using MAX_CONTEXT_TOKENS fallback"
            );
            config::env_parse("MAX_CONTEXT_TOKENS", 200_000)
        });
        let memory = match Memory::from_env().await {
            Ok(memory) => memory,
            Err(error) => {
                tracing::warn!(%error, "PostgreSQL memory unavailable, falling back to file-based memory");
                Memory::default()
            }
        };
        // Unlike memory, access control must not silently fall back to an
        // empty volatile store — that would forget configurers and per-user
        // policies (fail-open), so refuse to start instead.
        let bot_config_client = crate::bot_config::postgres_client_from_env()
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                "persistent access control initialization failed; refusing volatile fallback: {error}"
            )
            })?;
        let access_control = AccessControlStore::postgres(Arc::clone(&bot_config_client));
        let firecrawl = tools::firecrawl::Firecrawl::new(Arc::clone(&bot_config_client));
        let classifier_store = ClassifierStore::postgres(Arc::clone(&bot_config_client));
        let classifier = classifier_store
            .load()
            .await
            .map(|settings| Arc::new(classify::Classifier::new(settings)));
        let scheduler_limits = SchedulerLimitsStore::postgres(bot_config_client);
        let limits = scheduler_limits.load().await.unwrap_or(SchedulerLimits {
            max_inflight: config::env_parse(
                "MAX_INFLIGHT_LLM",
                housebot_llm_scheduler::DEFAULT_MAX_INFLIGHT,
            ),
        });
        let scheduler = Arc::new(LlmScheduler::new(limits.max_inflight));
        let scheduled_client = Arc::new(ScheduledChatClient::new(raw_client, scheduler));
        let client: Arc<dyn ChatClient> = scheduled_client.clone();
        let token_monitor = TokenMonitor::from_env().await.map_err(|error| {
            anyhow::anyhow!(
                "persistent token monitor initialization failed; refusing volatile fallback: {error}"
            )
        })?;
        Ok(Self {
            client,
            scheduled_client,
            model: config::env_or("LLM_MODEL", "gemma-4-26b-a4b-qat"),
            context_window_tokens,
            history: History::default(),
            memory,
            skills: Skills::default(),
            reminders: Reminders::default(),
            reporter: Arc::new(GitHubIssueReporter::default()),
            rate_limiter: tools::feature_request::default_rate_limiter(),
            feature_edit_limiter: tools::edit_feature_request::default_rate_limiter(),
            non_owner_dev_limiter: tools::feature_development::default_rate_limiter(),
            owner_dispatch_limiter: tools::feature_development::owner_dispatch_limiter(),
            pending_jobs: Arc::new(PendingJobStore::default()),
            searxng: Arc::new(SearxNg::from_env()),
            web_fetch: WebFetch::new(Some(firecrawl)),
            session_stats: tokio::sync::Mutex::new(HashMap::new()),
            token_monitor,
            active_conversations: tokio::sync::Mutex::new(HashMap::new()),
            access_control,
            scheduler_limits,
            classifier_store,
            classifier: std::sync::RwLock::new(classifier),
            discord,
            channel_context: ChannelContext::default(),
            sandbox_client: housebot_sandbox::SandboxClient::from_env(),
            merge_audit: tools::github_api::MergeAuditLog::default(),
        })
    }

    /// Current LLM scheduler utilization (active, pending, and both ceilings).
    /// Use this to decide whether to surface a queue-position message to users.
    pub fn llm_scheduler_info(&self) -> SchedulerInfo {
        self.scheduled_client.scheduler_info()
    }

    /// The shared LLM scheduler, for callers adjusting its limits at runtime.
    pub fn llm_scheduler(&self) -> &Arc<LlmScheduler> {
        self.scheduled_client.scheduler()
    }

    /// Access to the reminders store (the bot's delivery loop needs it).
    pub fn reminders(&self) -> &Reminders {
        &self.reminders
    }

    /// Shared persistent memory store used by the Discord command surface.
    pub fn memory(&self) -> Memory {
        self.memory.clone()
    }

    /// Shared bot-configuration access-control store (configurers + user policies).
    pub fn access_control(&self) -> AccessControlStore {
        self.access_control.clone()
    }

    /// Shared store persisting the scheduler ceilings across restarts.
    pub fn scheduler_limits(&self) -> SchedulerLimitsStore {
        self.scheduler_limits.clone()
    }

    /// Shared pending-job store; also held by `HouseBot` to drive the Discord component UI.
    pub fn pending_jobs(&self) -> Arc<PendingJobStore> {
        Arc::clone(&self.pending_jobs)
    }

    /// Access to the GitHub issue reporter (used by `HouseBot` for development job dispatch).
    pub fn reporter(&self) -> &GitHubIssueReporter {
        &self.reporter
    }
}

#[cfg(test)]
impl Agent {
    /// Construct an agent wired to a test client and temp-backed stores.
    pub fn for_test(
        client: Arc<dyn ChatClient>,
        history: History,
        memory: Memory,
        skills: Skills,
        reminders: Reminders,
    ) -> Self {
        let scheduler = Arc::new(LlmScheduler::default());
        let scheduled_client = Arc::new(ScheduledChatClient::new(client, scheduler));
        Self {
            client: scheduled_client.clone(),
            scheduled_client,
            model: "test-model".into(),
            context_window_tokens: 10_000,
            history,
            memory,
            skills,
            reminders,
            reporter: Arc::new(GitHubIssueReporter::new(
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            )),
            rate_limiter: tools::feature_request::default_rate_limiter(),
            feature_edit_limiter: tools::edit_feature_request::default_rate_limiter(),
            non_owner_dev_limiter: tools::feature_development::default_rate_limiter(),
            owner_dispatch_limiter: tools::feature_development::owner_dispatch_limiter(),
            pending_jobs: Arc::new(PendingJobStore::default()),
            searxng: Arc::new(SearxNg::from_env()),
            web_fetch: WebFetch::default(),
            session_stats: tokio::sync::Mutex::new(HashMap::new()),
            token_monitor: TokenMonitor::default(),
            active_conversations: tokio::sync::Mutex::new(HashMap::new()),
            access_control: AccessControlStore::default(),
            scheduler_limits: SchedulerLimitsStore::default(),
            classifier_store: ClassifierStore::default(),
            classifier: std::sync::RwLock::new(None),
            discord: Arc::new(DiscordBridge::default()),
            channel_context: ChannelContext::default(),
            sandbox_client: housebot_sandbox::SandboxClient::new("/dev/null"),
            merge_audit: tools::github_api::MergeAuditLog::default(),
        }
    }

    pub fn set_merge_audit_path(&mut self, path: impl Into<std::path::PathBuf>) {
        self.merge_audit = tools::github_api::MergeAuditLog::new(path);
    }

    pub fn set_max_context_tokens(&mut self, n: usize) {
        self.context_window_tokens = n;
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests_run.rs"]
mod tests_run;
