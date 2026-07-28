//! Constructing an `Agent` from the environment, including MCP bootstrap.

use super::*;

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
        // Unlike memory, access control must not silently fall back to an
        // empty volatile store — that would forget configurers and per-user
        // policies (fail-open), so refuse to start instead.
        let access_control = AccessControlStore::from_env().await.map_err(|error| {
            anyhow::anyhow!(
                "persistent access control initialization failed; refusing volatile fallback: {error}"
            )
        })?;
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
            user_config: UserConfigStore::default(),
            discord,
            channel_log: ChannelLog::default(),
            sandbox_client: housebot_sandbox::SandboxClient::from_env(),
            merge_audit: tools::github_api::MergeAuditLog::default(),
        })
    }
}

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
