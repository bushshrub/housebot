//! The agentic loop: builds prompts, streams completions from the LLM, dispatches tool
//! calls (built-in tools + MCP servers), and persists per-user history and memory.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Local, Utc};
use serde_json::{json, Value};

use crate::bot_config::{AccessControl, AccessControlStore};
use crate::channel_log::ChannelLog;
use crate::coding_agent::pending::PendingJobStore;
use crate::config;
use crate::discord_bridge::DiscordBridge;
use crate::github_issues::GitHubIssueReporter;
use crate::history::History;
use crate::llm::{ChatClient, OpenAiClient, TextSink, ThinkingMode, TokenUsage};
use crate::llm_queue::{LlmPriority, LlmQueueInfo, LlmRequestQueue, QueuedChatClient};
use crate::lua_engine::{self, ScriptHost};
use crate::mcp::McpServer;
use crate::memory::Memory;
use crate::profile::ProfileStore;
use crate::rate_limit::RateLimiter;
use crate::reminders::Reminders;
use crate::skills::{Skill, Skills};
use crate::token_monitor::{
    LeaderboardEntry, LeaderboardMetric, LeaderboardPeriod, TokenLeaderboard, TokenMonitor,
};
use crate::tool_permissions::ToolPermissions;
use crate::tools;
use crate::tools::common_crawl::CommonCrawl;
use crate::tools::file_download::FileDownloader;
use crate::tools::sandbox::LazySandbox;
use crate::tools::searxng::SearxNg;
use crate::tools::web_fetch::WebFetch;

/// An inbound media attachment, base64-encoded for the multimodal API.
#[derive(Debug, Clone)]
pub struct MediaData {
    pub media_type: String,
    pub data: String,
}

/// One user turn to run through the agent.
#[derive(Debug, Clone, Copy)]
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
        }
    }
}

/// Structured bot-control action extracted from a tool call, carried alongside text.
#[derive(Debug, Clone)]
pub enum AgentControlAction {
    /// Owner immediate dispatch is ready.
    OwnerDispatchReady { job_id: uuid::Uuid },
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

/// The agent: LLM client, storage, tools, and connected MCP servers.
pub struct Agent {
    client: Arc<dyn ChatClient>,
    queued_client: Arc<QueuedChatClient>,
    model: String,
    context_window_tokens: usize,
    history: History,
    memory: Memory,
    profile_store: ProfileStore,
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
    file_downloader: FileDownloader,
    common_crawl: CommonCrawl,
    mcp_servers: Arc<Vec<McpServer>>,
    session_stats: tokio::sync::Mutex<HashMap<String, SessionStats>>,
    token_monitor: TokenMonitor,
    active_conversations: tokio::sync::Mutex<HashMap<String, String>>,
    tool_permissions: ToolPermissions,
    access_control: AccessControlStore,
    discord: Arc<DiscordBridge>,
    channel_log: ChannelLog,
    sandbox_client: housebot_sandbox::SandboxClient,
}

mod dispatch;
mod leaderboard_fmt;
mod lua;
pub use lua::BotScriptHost;
mod prompt;
mod run;
mod session;
mod tools_def;

#[allow(unused_imports)]
use leaderboard_fmt::*;
#[allow(unused_imports)]
use lua::*;
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
        let mcp_servers = Arc::new(start_mcp_servers().await);
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
        let queue = Arc::new(LlmRequestQueue::default());
        let queued_client = Arc::new(QueuedChatClient::new(raw_client, queue));
        let client: Arc<dyn ChatClient> = queued_client.clone();
        let memory = match Memory::from_env().await {
            Ok(memory) => memory,
            Err(error) => {
                tracing::warn!(%error, "PostgreSQL memory unavailable, falling back to file-based memory");
                Memory::default()
            }
        };
        let access_control = match AccessControlStore::from_env().await {
            Ok(store) => store,
            Err(error) => {
                tracing::warn!(%error, "PostgreSQL bot config unavailable, falling back to file-based access control");
                AccessControlStore::default()
            }
        };
        let token_monitor = TokenMonitor::from_env().await.map_err(|error| {
            anyhow::anyhow!(
                "persistent token monitor initialization failed; refusing volatile fallback: {error}"
            )
        })?;
        Ok(Self {
            client,
            queued_client,
            model: config::env_or("LLM_MODEL", "gemma-4-12b-qat-q4kxl"),
            context_window_tokens,
            history: History::default(),
            memory,
            profile_store: ProfileStore::default(),
            skills: Skills::default(),
            reminders: Reminders::default(),
            reporter: Arc::new(GitHubIssueReporter::default()),
            rate_limiter: tools::feature_request::default_rate_limiter(),
            feature_edit_limiter: tools::edit_feature_request::default_rate_limiter(),
            non_owner_dev_limiter: tools::feature_development::default_rate_limiter(),
            owner_dispatch_limiter: tools::feature_development::owner_dispatch_limiter(),
            pending_jobs: Arc::new(PendingJobStore::default()),
            searxng: Arc::new(SearxNg::from_env()),
            web_fetch: WebFetch::default(),
            file_downloader: FileDownloader::default(),
            common_crawl: CommonCrawl::default(),
            mcp_servers,
            session_stats: tokio::sync::Mutex::new(HashMap::new()),
            token_monitor,
            active_conversations: tokio::sync::Mutex::new(HashMap::new()),
            tool_permissions: ToolPermissions::default(),
            access_control,
            discord,
            channel_log: ChannelLog::default(),
            sandbox_client: housebot_sandbox::SandboxClient::from_env(),
        })
    }

    /// Current LLM queue utilization (active, pending, capacity).
    /// Use this to decide whether to surface a queue-position message to users.
    pub fn llm_queue_info(&self) -> LlmQueueInfo {
        self.queued_client.queue_info()
    }

    /// Access to the reminders store (the bot's delivery loop needs it).
    pub fn reminders(&self) -> &Reminders {
        &self.reminders
    }

    /// Shared persistent memory store used by the Discord command surface.
    pub fn memory(&self) -> Memory {
        self.memory.clone()
    }

    /// Shared guild-scoped tool permission store used by Discord commands.
    pub fn tool_permissions(&self) -> ToolPermissions {
        self.tool_permissions.clone()
    }

    /// Shared bot-configuration access-control store (configurers + user policies).
    pub fn access_control(&self) -> AccessControlStore {
        self.access_control.clone()
    }

    /// Shared pending-job store; also held by `HouseBot` to drive the Discord component UI.
    pub fn pending_jobs(&self) -> Arc<PendingJobStore> {
        Arc::clone(&self.pending_jobs)
    }

    /// Access to the GitHub issue reporter (used by `HouseBot` for development job dispatch).
    pub fn reporter(&self) -> &GitHubIssueReporter {
        &self.reporter
    }

    /// Web search for the Lua scripting engine — same SearXNG instance and
    /// rate limits as the agent's `web_search` tool.
    pub async fn web_search(&self, query: &str, max_results: usize) -> String {
        self.searxng
            .search(query, max_results.clamp(1, 20), "")
            .await
    }

    /// Search Jellyfin for the Lua scripting engine, via the MCP server's
    /// search tool (matched by name, since the tool set is server-defined).
    pub async fn jellyfin_search(&self, query: &str) -> String {
        let Some(server) = self.mcp_servers.iter().find(|s| s.prefix == "jellyfin") else {
            return "Error: Jellyfin is not available.".to_string();
        };
        let tools = server.list_tools().await;
        let Some(tool) = tools.iter().find(|t| t.name == "search") else {
            return "Error: the Jellyfin server exposes no search tool.".to_string();
        };
        match server.call_tool(&tool.name, json!({"query": query})).await {
            Ok(text) => text,
            Err(e) => format!("Error: {e}"),
        }
    }
}

// ── MCP server configuration ─────────────────────────────────────────────────

async fn start_mcp_servers() -> Vec<McpServer> {
    let mut servers = Vec::new();
    match (
        std::env::var("JELLYFIN_URL"),
        std::env::var("JELLYFIN_API_KEY"),
    ) {
        (Ok(url), Ok(key)) if !url.is_empty() && !key.is_empty() => {
            if let Some(s) = McpServer::start(
                "jellyfin",
                "jellyfin-mcp",
                &["--read-only".to_string()],
                &[
                    ("JELLYFIN_URL".into(), url),
                    ("JELLYFIN_API_KEY".into(), key),
                ],
            )
            .await
            {
                servers.push(s);
            }
        }
        _ => tracing::warn!("JELLYFIN_URL or JELLYFIN_API_KEY not set — Jellyfin MCP disabled"),
    }
    servers
}

#[cfg(test)]
impl Agent {
    /// Construct an agent wired to a test client and temp-backed stores.
    pub fn for_test(
        client: Arc<dyn ChatClient>,
        history: History,
        memory: Memory,
        profile_store: ProfileStore,
        skills: Skills,
        reminders: Reminders,
    ) -> Self {
        let queue = Arc::new(LlmRequestQueue::default());
        let queued_client = Arc::new(QueuedChatClient::new(client, queue));
        Self {
            client: queued_client.clone(),
            queued_client,
            model: "test-model".into(),
            context_window_tokens: 10_000,
            history,
            memory,
            profile_store,
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
            file_downloader: FileDownloader::default(),
            common_crawl: CommonCrawl::default(),
            mcp_servers: Arc::new(vec![]),
            session_stats: tokio::sync::Mutex::new(HashMap::new()),
            token_monitor: TokenMonitor::default(),
            active_conversations: tokio::sync::Mutex::new(HashMap::new()),
            tool_permissions: ToolPermissions::default(),
            access_control: AccessControlStore::default(),
            discord: Arc::new(DiscordBridge::default()),
            channel_log: ChannelLog::default(),
            sandbox_client: housebot_sandbox::SandboxClient::new("/dev/null"),
        }
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
