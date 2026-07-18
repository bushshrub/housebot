//! Discord interface (serenity): message routing, `!`-commands, streaming progress
//! updates, secret redaction, and code file uploads.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use regex::Regex;
use serenity::all::{
    ButtonStyle, ChannelId, Command, CommandDataOptionValue, CommandOptionType,
    ComponentInteractionDataKind, Context, CreateActionRow, CreateAllowedMentions,
    CreateAttachment, CreateAutocompleteResponse, CreateButton, CreateCommand, CreateCommandOption,
    CreateEmbed, CreateInteractionResponse, CreateInteractionResponseMessage, CreateSelectMenu,
    CreateSelectMenuKind, CreateSelectMenuOption, EditInteractionResponse, EditMessage,
    EventHandler, GatewayIntents, GuildId, Interaction, Message, Ready, UserId,
};
use serenity::builder::CreateMessage;
use serenity::Client;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::agent::{
    Agent, AgentControlAction, AgentHooks, AgentRequest, AgentResult, MediaData, NoHooks,
};
use crate::bot_config::{LeaderboardVisibility, ServerConfig, ServerConfigStore, UserConfigStore};
pub use crate::bot_response::SecretRedactor;
use crate::channel_log::ChannelLog;
use crate::coding_agent::catalog::{AgentCatalog, CodingAgent};
use crate::coding_agent::issue::{
    build_dispatch_prompt, build_issue_body, dispatch_labels, DISPATCH_WORKFLOW_FILE,
};
use crate::coding_agent::pending::{DiscordMessageRef, DispatchStage, PendingJobStore};
use crate::config;
use crate::discord_bridge::DiscordBridge;
use crate::graph_render;
use crate::grocery::GroceryList;
use crate::history::History;
use crate::llm::ThinkingMode;
use crate::lua_engine;
use crate::memory::Memory;
use crate::message_log::MessageLog;
use crate::notes::Notes;
use crate::profile::ProfileStore;
use crate::rate_limit::RateLimiter;
use crate::skills::Skills;
use crate::token_monitor::{LeaderboardMetric, LeaderboardPeriod};
use crate::tool_permissions::{ToolPermissions, VoteResult};

pub use crate::bot_commands::{
    erase_data_command, grocery_command, memory_command, note_command, skill_command, stats_command,
};
use crate::bot_formatting::{append_citations, append_tool_summary, tool_status};
pub use crate::bot_formatting::{extract_code_files, lang_ext, split_text, tool_hint};

const MAX_MESSAGE_LENGTH: usize = 2000;
const EMBED_DESCRIPTION_LIMIT: usize = 4096;
const PAGINATION_PREFIX: &str = "housebot_labs_page:";
const DEVELOP_PREFIX: &str = "develop:";
/// How often, and past what age, stray `/lua` graph scratch files are swept
/// from the temp dir. Normal renders clean up immediately (see
/// `graph_render::TempFileGuard`); this only catches leaks from a hard
/// crash or an older build.
const GRAPH_SWEEP_INTERVAL: Duration = Duration::from_secs(600);
const GRAPH_SWEEP_MAX_AGE: Duration = Duration::from_secs(600);

struct PaginatedResponse {
    owner_id: u64,
    pages: Vec<String>,
}

mod command_defs;
mod config_cmd;
mod develop;
mod develop_actions;
mod develop_component;
pub(crate) mod emoji_reactions;
mod handler;
mod helpers;
mod interactions;
mod lua_cmd;
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
    notes: Notes,
    skills: Skills,
    memory: Memory,
    history: History,
    profile_store: ProfileStore,
    message_log: MessageLog,
    server_cfg: ServerConfigStore,
    user_cfg: UserConfigStore,
    conversations: Mutex<ConversationTracker>,
    processing: Mutex<HashSet<u64>>,
    responded: Mutex<VecDeque<u64>>,
    proactive_cooldowns: Mutex<HashMap<(u64, u64), Instant>>,
    paginated: Mutex<HashMap<String, PaginatedResponse>>,
    reminder_started: AtomicBool,
    graph_sweep_started: AtomicBool,
    chat_rate_limiter: RateLimiter,
    lua_rate_limiter: RateLimiter,
    /// Shared with `Agent` — holds pending coding-agent dispatch jobs.
    pending_jobs: Arc<PendingJobStore>,
    /// Catalog of agents, models, and effort levels.
    catalog: AgentCatalog,
    /// Shared with `Agent` — provides Discord API access to the agent tools.
    discord: Arc<DiscordBridge>,
    /// Per-user grocery lists.
    grocery: GroceryList,
    /// Logs all guild channel messages for the search_messages tool.
    channel_log: ChannelLog,
}

impl HouseBot {
    /// Build the bot from environment configuration.
    pub fn new(agent: Arc<Agent>, discord: Arc<DiscordBridge>) -> Self {
        let idle = Duration::from_secs(config::env_parse("CONVERSATION_IDLE_TIMEOUT", 300));
        let chat_rate_max: usize = config::env_parse("CHAT_RATE_LIMIT_MAX", 20);
        let chat_rate_window =
            Duration::from_secs(config::env_parse("CHAT_RATE_LIMIT_WINDOW_SECS", 60u64));
        let pending_jobs = agent.pending_jobs();
        let memory = agent.memory();
        Self {
            agent,
            redactor: Arc::new(SecretRedactor::from_env()),
            notes: Notes::default(),
            skills: Skills::default(),
            memory,
            history: History::default(),
            profile_store: ProfileStore::default(),
            message_log: MessageLog::default(),
            server_cfg: ServerConfigStore::default(),
            user_cfg: UserConfigStore::default(),
            conversations: Mutex::new(ConversationTracker::new(idle)),
            processing: Mutex::new(HashSet::new()),
            responded: Mutex::new(VecDeque::with_capacity(200)),
            proactive_cooldowns: Mutex::new(HashMap::new()),
            paginated: Mutex::new(HashMap::new()),
            reminder_started: AtomicBool::new(false),
            graph_sweep_started: AtomicBool::new(false),
            chat_rate_limiter: RateLimiter::new(chat_rate_max, chat_rate_window),
            lua_rate_limiter: RateLimiter::new(
                config::env_parse("LUA_RATE_LIMIT_MAX", 6),
                Duration::from_secs(config::env_parse("LUA_RATE_LIMIT_WINDOW_SECS", 60u64)),
            ),
            pending_jobs,
            catalog: AgentCatalog::load_embedded(),
            grocery: GroceryList::default(),
            discord,
            channel_log: ChannelLog::default(),
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
        let name = self
            .profile_store
            .load(user_id)
            .await
            .best_name()
            .to_string();
        format!("New conversation started, {name}. Your previous conversation history has been cleared.")
    }

    pub(crate) async fn proactive_cooldown_allows(&self, channel_id: u64, user_id: u64) -> bool {
        let now = Instant::now();
        let cooldown = Duration::from_secs(config::env_parse(
            "PROACTIVE_ASSISTANCE_COOLDOWN_SECS",
            300u64,
        ));
        let mut cooldowns = self.proactive_cooldowns.lock().await;
        if cooldowns
            .get(&(channel_id, user_id))
            .is_some_and(|last| now.duration_since(*last) < cooldown)
        {
            return false;
        }
        cooldowns.insert((channel_id, user_id), now);
        true
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
    let bot = HouseBot::new(agent, discord);

    let intents = GatewayIntents::non_privileged() | GatewayIntents::MESSAGE_CONTENT;
    let mut client = Client::builder(&token, intents).event_handler(bot).await?;
    tracing::info!("Agent and MCP servers ready");
    client.start().await?;
    Ok(())
}

#[cfg(test)]
#[path = "bot_tests.rs"]
mod tests;
