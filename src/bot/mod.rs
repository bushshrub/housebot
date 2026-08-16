//! Discord interface (serenity): message routing, `!`-commands, streaming progress
//! updates, secret redaction, and code file uploads.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use regex::Regex;
use serenity::all::{
    ButtonStyle, Command, CommandDataOptionValue, CommandOptionType, ComponentInteractionDataKind,
    Context, CreateActionRow, CreateAllowedMentions, CreateAttachment, CreateButton, CreateCommand,
    CreateCommandOption, CreateEmbed, CreateInteractionResponse, CreateInteractionResponseMessage,
    CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption, EditInteractionResponse,
    EditMessage, EventHandler, GatewayIntents, GuildId, Interaction, Message, Ready, UserId,
};
use serenity::builder::CreateMessage;
use serenity::Client;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::agent::{
    Agent, AgentControlAction, AgentHooks, AgentRequest, AgentResult, CancelToken, MediaData,
    NoHooks,
};
use crate::bot_config::{
    AccessControlStore, LeaderboardVisibility, SchedulerLimits, SchedulerLimitsStore, ServerConfig,
    ServerConfigStore, UserConfigStore,
};
pub use crate::bot_response::SecretRedactor;
use crate::channel_context::ChannelContext;
use crate::coding_agent::catalog::{AgentCatalog, CodingAgent};
use crate::coding_agent::issue::{dispatch_inputs, dispatch_workflow_file};
use crate::coding_agent::pending::{DiscordMessageRef, DispatchStage, PendingJobStore};
use crate::config;
use crate::discord_bridge::DiscordBridge;
use crate::history::History;
use crate::llm::ThinkingMode;
use crate::llm_scheduler::LlmScheduler;
use crate::memory::Memory;
use crate::rate_limit::RateLimiter;
use crate::skills::Skills;
use crate::token_monitor::{LeaderboardMetric, LeaderboardPeriod};

pub use crate::bot_commands::{
    erase_data_command, memory_command, skill_command, skill_delete, skill_info, skill_list,
    stats_command,
};
use crate::bot_formatting::{append_tool_summary, tool_status};
pub use crate::bot_formatting::{extract_code_files, lang_ext, split_text, tool_hint};

const MAX_MESSAGE_LENGTH: usize = 2000;
const EMBED_DESCRIPTION_LIMIT: usize = 4096;
const PAGINATION_PREFIX: &str = "housebot_labs_page:";
const DEVELOP_PREFIX: &str = "develop:";

struct PaginatedResponse {
    owner_id: u64,
    pages: Vec<String>,
}

mod command_defs;
mod config_cmd;
mod develop;
mod develop_actions;
mod develop_component;
mod handler;
mod helpers;
mod interactions;
mod media;
mod message_flow;
mod progress;
mod render;
#[allow(unused_imports)]
use command_defs::*;
#[allow(unused_imports)]
use config_cmd::*;
#[allow(unused_imports)]
use develop::*;
#[allow(unused_imports)]
use develop_actions::*;
#[allow(unused_imports)]
use develop_component::*;
#[allow(unused_imports)]
use helpers::*;
#[allow(unused_imports)]
use interactions::*;
#[allow(unused_imports)]
use media::*;
#[allow(unused_imports)]
use progress::*;
#[allow(unused_imports)]
use render::*;

// ── pure helpers ─────────────────────────────────────────────────────────────

static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://[^\s<>]+|www\.[^\s<>]+").unwrap());

/// Tracks which (channel, user) conversations are still within the idle window.
pub struct ConversationTracker {
    default_idle_timeout: Duration,
    last_active: std::collections::HashMap<(u64, u64), (Instant, Duration)>,
}

impl ConversationTracker {
    pub fn new(idle_timeout: Duration) -> Self {
        Self {
            default_idle_timeout: idle_timeout,
            last_active: std::collections::HashMap::new(),
        }
    }

    pub fn is_active(&self, channel_id: u64, user_id: u64, now: Instant) -> bool {
        match self.last_active.get(&(channel_id, user_id)) {
            Some(&(t, timeout)) => now.duration_since(t) <= timeout,
            None => false,
        }
    }

    /// Remove an expired entry; return whether one existed.
    pub fn pop_timed_out(&mut self, channel_id: u64, user_id: u64, now: Instant) -> bool {
        let key = (channel_id, user_id);
        if let Some(&(t, timeout)) = self.last_active.get(&key) {
            if now.duration_since(t) > timeout {
                self.last_active.remove(&key);
                return true;
            }
        }
        false
    }

    pub fn mark_active(&mut self, channel_id: u64, user_id: u64, now: Instant, timeout: Duration) {
        self.last_active
            .insert((channel_id, user_id), (now, timeout));
    }

    pub fn remove(&mut self, channel_id: u64, user_id: u64) {
        self.last_active.remove(&(channel_id, user_id));
    }

    pub fn default_timeout(&self) -> Duration {
        self.default_idle_timeout
    }
}

// ── serenity handler ─────────────────────────────────────────────────────────

/// The Discord client state.
pub struct HouseBot {
    agent: Arc<Agent>,
    redactor: Arc<SecretRedactor>,
    skills: Skills,
    memory: Memory,
    history: History,
    server_cfg: ServerConfigStore,
    user_cfg: UserConfigStore,
    /// Shared with `Agent` — configurer allowlist and per-user policies.
    access: AccessControlStore,
    conversations: Mutex<ConversationTracker>,
    processing: Mutex<HashSet<u64>>,
    responded: Mutex<VecDeque<u64>>,
    paginated: Mutex<HashMap<String, PaginatedResponse>>,
    reminder_started: AtomicBool,
    chat_rate_limiter: RateLimiter,
    /// Shared with `Agent` — holds pending coding-agent dispatch jobs.
    pending_jobs: Arc<PendingJobStore>,
    /// Catalog of agents and models.
    catalog: AgentCatalog,
    /// Shared with `Agent` — provides Discord API access to the agent tools.
    discord: Arc<DiscordBridge>,
    /// Logs all guild channel messages for the get_messages tool's search mode.
    channel_context: ChannelContext,
    /// Tracks active progress messages so the ❌ cancel reaction can be
    /// matched to the right user and agent run.  Keyed by progress message ID.
    progress_messages: Arc<Mutex<HashMap<u64, (u64, CancelToken)>>>,
}

impl HouseBot {
    /// Build the bot from environment configuration.
    pub async fn new(agent: Arc<Agent>, discord: Arc<DiscordBridge>) -> Self {
        let idle = Duration::from_secs(config::env_parse("CONVERSATION_IDLE_TIMEOUT", 300));
        let chat_rate_max: usize = config::env_parse("CHAT_RATE_LIMIT_MAX", 20);
        let chat_rate_window =
            Duration::from_secs(config::env_parse("CHAT_RATE_LIMIT_WINDOW_SECS", 60u64));
        let pending_jobs = agent.pending_jobs();
        let memory = agent.memory();
        let access = agent.access_control();
        let (server_cfg, user_cfg) = match crate::bot_config::postgres_client_from_env().await {
            Ok(client) => (
                ServerConfigStore::postgres(Arc::clone(&client)).await,
                UserConfigStore::postgres(client).await,
            ),
            Err(error) => {
                tracing::warn!(%error, "PostgreSQL bot config unavailable, falling back to file-based server/user config");
                (ServerConfigStore::default(), UserConfigStore::default())
            }
        };
        Self {
            agent,
            redactor: Arc::new(SecretRedactor::from_env()),
            skills: Skills::default(),
            memory,
            history: History::default(),
            server_cfg,
            user_cfg,
            access,
            conversations: Mutex::new(ConversationTracker::new(idle)),
            processing: Mutex::new(HashSet::new()),
            responded: Mutex::new(VecDeque::with_capacity(200)),
            paginated: Mutex::new(HashMap::new()),
            reminder_started: AtomicBool::new(false),
            chat_rate_limiter: RateLimiter::new(chat_rate_max, chat_rate_window),
            pending_jobs,
            catalog: AgentCatalog::load_embedded(),
            discord,
            channel_context: ChannelContext::default(),
            progress_messages: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) async fn already_seen(&self, id: u64) -> bool {
        let mut processing = self.processing.lock().await;
        let responded = self.responded.lock().await;
        if processing.contains(&id) || responded.contains(&id) {
            return true;
        }
        processing.insert(id);
        false
    }

    pub(crate) async fn mark_done(&self, id: u64) {
        self.processing.lock().await.remove(&id);
        let mut responded = self.responded.lock().await;
        if responded.len() >= 200 {
            responded.pop_front();
        }
        responded.push_back(id);
    }

    /// Start a fresh conversation for `/session new`.
    pub(crate) async fn handle_new(&self, channel_id: u64, user_id: u64) -> String {
        tracing::info!(target: "housebot::commands", user_id, "Session reset requested");
        self.agent.reset_session(&user_id.to_string()).await;
        self.conversations.lock().await.remove(channel_id, user_id);
        "New conversation started. Your previous conversation history has been cleared.".to_string()
    }

    pub(crate) async fn respond(&self, ctx: &Context, msg: &Message, content: &str) {
        let _ = reply_no_ping(ctx, msg, content).await;
    }
}

/// Run the bot: build the agent, register the handler, and connect to Discord.
pub async fn run() -> anyhow::Result<()> {
    let token = std::env::var("DISCORD_BOT_TOKEN")
        .map_err(|_| anyhow::anyhow!("DISCORD_BOT_TOKEN is not set"))?;
    let discord = Arc::new(DiscordBridge::default());
    let agent = Arc::new(Agent::from_env(discord.clone()).await?);
    let bot = HouseBot::new(agent, discord).await;

    let intents = GatewayIntents::non_privileged() | GatewayIntents::MESSAGE_CONTENT;
    let mut client = Client::builder(&token, intents).event_handler(bot).await?;
    tracing::info!("Agent and MCP servers ready");
    client.start().await?;
    Ok(())
}

#[cfg(test)]
#[path = "bot_tests.rs"]
mod tests;
