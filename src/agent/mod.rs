//! The agentic loop: builds prompts, streams completions from the LLM, dispatches tool
//! calls (built-in tools + MCP servers), and persists per-user history and memory.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Local, Utc};
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::bot_config::{AccessControl, AccessControlStore, UserConfigStore};
use crate::channel_log::ChannelLog;
use crate::coding_agent::pending::PendingJobStore;
use crate::config;
use crate::discord_bridge::DiscordBridge;
use crate::github_issues::GitHubIssueReporter;
use crate::history::History;
use crate::llm::{ChatClient, OpenAiClient, TextSink, ThinkingMode, TokenUsage};
use crate::llm_queue::{LlmQueueInfo, LlmRequestQueue, QueuedChatClient};
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

mod emoji;
mod startup;
mod types;

pub(crate) use emoji::*;
pub use types::*;

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
    /// Per-user configuration, including each user's enabled marketplace skills.
    user_config: UserConfigStore,
    discord: Arc<DiscordBridge>,
    channel_log: ChannelLog,
    sandbox_client: housebot_sandbox::SandboxClient,
    /// Audit trail of administrator pull-request merges.
    merge_audit: tools::github_api::MergeAuditLog,
}

mod configure_bot;
mod dispatch;
mod dispatch_discord;
mod dispatch_features;
mod dispatch_lua;
mod dispatch_sandbox;
mod dispatch_skills;
mod dispatch_web;
mod leaderboard_fmt;
mod lua;
pub use lua::BotScriptHost;
mod prompt;
mod prompt_base;
mod prompt_suffix;
mod run;
mod session;
mod tool_exec;
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

    /// Ask the model whether an incoming mention should receive a single emoji
    /// instead of a full agent response.
    /// Returns `None` when the model is unreachable or the response is empty.
    pub async fn select_emoji(&self, text: &str) -> Option<String> {
        let prompt = format!(
            "Decide whether this message can be fully answered by one emoji reaction. \
             Use an emoji only for lightweight greetings, thanks, jokes, social \
             acknowledgements, or similarly low-stakes messages requiring no information \
             or action. For questions, requests, commands, ambiguous messages, or anything \
             needing a substantive response, return NONE. Respond with exactly one emoji \
             or NONE.\n\nMessage:\n{text}"
        );
        let messages = vec![
            json!({"role": "system", "content": "Choose one emoji-only response or NONE. Never add explanation."}),
            json!({"role": "user", "content": prompt}),
        ];
        let start = std::time::Instant::now();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            self.queued_client.chat_once(&self.model, &messages, 128),
        )
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("emoji selection timed out")));
        match result {
            Ok(completion) => {
                let elapsed = start.elapsed();
                let emoji = completion
                    .content
                    .as_deref()
                    .and_then(parse_emoji_selection);
                tracing::debug!(
                    target: "housebot::emoji",
                    text_chars = text.chars().count(),
                    selected = ?emoji,
                    elapsed_ms = elapsed.as_millis() as u64,
                    "Emoji selection complete"
                );
                emoji
            }
            Err(error) => {
                tracing::warn!(
                    target: "housebot::emoji",
                    %error,
                    "Emoji selection LLM call failed"
                );
                None
            }
        }
    }
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
            user_config: UserConfigStore::default(),
            discord: Arc::new(DiscordBridge::default()),
            channel_log: ChannelLog::default(),
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
#[path = "tests_core.rs"]
mod tests_core;
#[cfg(test)]
#[path = "tests_formatting.rs"]
mod tests_formatting;
#[cfg(test)]
#[path = "tests_leaderboard.rs"]
mod tests_leaderboard;
#[cfg(test)]
#[path = "tests_support.rs"]
mod tests_support;

#[cfg(test)]
#[path = "tests_run_dispatch.rs"]
mod tests_run_dispatch;
#[cfg(test)]
#[path = "tests_run_persistence.rs"]
mod tests_run_persistence;
#[cfg(test)]
#[path = "tests_run_stream.rs"]
mod tests_run_stream;
#[cfg(test)]
#[path = "tests_run_support.rs"]
mod tests_run_support;
