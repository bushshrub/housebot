//! Discord interface (serenity): message routing, `!`-commands, streaming progress
//! updates, secret redaction, and code/artifact file uploads.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::{Captures, Regex};
use serde_json::Value;
use serenity::all::{
    ButtonStyle, Command, CommandDataOptionValue, CommandOptionType, Context, CreateActionRow,
    CreateAllowedMentions, CreateAttachment, CreateButton, CreateCommand, CreateCommandOption,
    CreateEmbed, CreateInteractionResponse, CreateInteractionResponseMessage,
    EditInteractionResponse, EditMessage, EventHandler, GatewayIntents, Interaction, Message,
    Ready, UserId,
};
use serenity::builder::CreateMessage;
use serenity::Client;
use tokio::sync::Mutex;

use crate::agent::{Agent, AgentHooks, AgentResult, ImageData, NoHooks};
use crate::bot_config::{ServerConfigStore, UserConfigStore};
pub use crate::bot_response::SecretRedactor;
use crate::config;
use crate::history::History;
use crate::memory::Memory;
use crate::notes::Notes;
use crate::skills::Skills;

pub use crate::bot_commands::{note_command, skill_command, stats_command};

const MAX_MESSAGE_LENGTH: usize = 2000;
const CODE_FILE_THRESHOLD: usize = 800;
const EMBED_DESCRIPTION_LIMIT: usize = 4096;
const PAGINATION_PREFIX: &str = "housebot_labs_page:";

struct PaginatedResponse {
    owner_id: u64,
    pages: Vec<String>,
}

fn compact_progress(stage: usize, detail: Option<&str>) -> String {
    let filled = (stage / 10).min(10);
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled));
    match detail {
        Some(detail) => format!("🧠 **Compacting conversation**\n`[{bar}] {stage}%` — {detail}"),
        None => format!("🧠 **Compacting conversation**\n`[{bar}] {stage}%`"),
    }
}

enum CompactProgressTarget {
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

struct CompactProgressHooks(CompactProgressTarget);

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

struct ResponseProgressHooks {
    ctx: Context,
    channel_id: serenity::all::ChannelId,
    message_id: serenity::all::MessageId,
    generating: AtomicBool,
}

impl ResponseProgressHooks {
    fn new(ctx: &Context, progress: &Message) -> Self {
        Self {
            ctx: ctx.clone(),
            channel_id: progress.channel_id,
            message_id: progress.id,
            generating: AtomicBool::new(false),
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
}
// ── pure helpers ─────────────────────────────────────────────────────────────

/// Map a fenced-code language tag to a file extension.
pub fn lang_ext(lang: &str) -> &'static str {
    match lang {
        "python" | "py" => ".py",
        "javascript" | "js" => ".js",
        "typescript" | "ts" => ".ts",
        "bash" | "sh" | "shell" => ".sh",
        "rust" => ".rs",
        "go" => ".go",
        "java" => ".java",
        "c" => ".c",
        "cpp" | "c++" => ".cpp",
        "html" => ".html",
        "css" => ".css",
        "json" => ".json",
        "yaml" | "yml" => ".yaml",
        "toml" => ".toml",
        "sql" => ".sql",
        "ruby" | "rb" => ".rb",
        "php" => ".php",
        _ => ".txt",
    }
}

fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Split `text` into chunks no longer than `limit` characters, preferring newline breaks.
pub fn split_text(text: &str, limit: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        if chars.len() - start <= limit {
            chunks.push(chars[start..].iter().collect());
            break;
        }
        let window_end = start + limit;
        let mut split = window_end;
        for i in (start..window_end).rev() {
            if chars[i] == '\n' {
                split = i;
                break;
            }
        }
        if split <= start {
            split = window_end;
        }
        chunks.push(chars[start..split].iter().collect());
        let mut next = split;
        while next < chars.len() && chars[next] == '\n' {
            next += 1;
        }
        start = next;
    }
    chunks
}

/// Return a short human-readable suffix describing a tool call's arguments.
pub fn tool_hint(tool_name: &str, args: &Value) -> String {
    let get = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("");
    match tool_name {
        "run_skill" => {
            let name = get("name");
            if name.is_empty() {
                return String::new();
            }
            let inp = truncate_chars(get("input"), 60).replace('\n', " ");
            format!(" — {name}: {inp}")
        }
        "set_reminder" => {
            let msg = get("message");
            if msg.is_empty() {
                return String::new();
            }
            let msg = truncate_chars(msg, 60).replace('\n', " ");
            let delay = args
                .get("delay_minutes")
                .map(|d| d.to_string())
                .unwrap_or_default();
            format!(" — in {delay}m: {msg}")
        }
        "translate" => {
            let lang = get("target_language");
            if lang.is_empty() {
                return String::new();
            }
            let txt = truncate_chars(get("text"), 40).replace('\n', " ");
            format!(" — → {lang}: {txt}")
        }
        _ => {
            for key in ["query", "task", "repo_url", "memory_content", "url"] {
                let val = get(key);
                if !val.is_empty() {
                    let mut preview = truncate_chars(val, 80).replace('\n', " ");
                    if val.chars().count() > 80 {
                        preview.push('…');
                    }
                    return format!(" — {preview}");
                }
            }
            String::new()
        }
    }
}

static CODE_FENCE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)```(\w*)\n(.*?)(?:```|$)").unwrap());
static URL: Lazy<Regex> = Lazy::new(|| Regex::new(r"https?://[^\s<>]+|www\.[^\s<>]+").unwrap());

/// Replace large fenced code blocks with file references; return modified text + files.
pub fn extract_code_files(text: &str) -> (String, Vec<(String, Vec<u8>)>) {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut counter = 0u32;
    let modified = CODE_FENCE.replace_all(text, |caps: &Captures| {
        let lang = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_lowercase();
        let code = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        if code.chars().count() < CODE_FILE_THRESHOLD {
            return caps.get(0).unwrap().as_str().to_string();
        }
        counter += 1;
        let filename = format!("script_{counter}{}", lang_ext(&lang));
        files.push((filename.clone(), code.as_bytes().to_vec()));
        format!("*(see attached: `{filename}`)*")
    });
    (modified.into_owned(), files)
}

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
    server_cfg: ServerConfigStore,
    user_cfg: UserConfigStore,
    conversations: Mutex<ConversationTracker>,
    processing: Mutex<HashSet<u64>>,
    responded: Mutex<VecDeque<u64>>,
    paginated: Mutex<HashMap<String, PaginatedResponse>>,
    reminder_started: AtomicBool,
}

impl HouseBot {
    /// Build the bot from environment configuration.
    pub fn new(agent: Arc<Agent>) -> Self {
        let idle = Duration::from_secs(config::env_parse("CONVERSATION_IDLE_TIMEOUT", 300));
        Self {
            agent,
            redactor: Arc::new(SecretRedactor::from_env()),
            notes: Notes::default(),
            skills: Skills::default(),
            memory: Memory::default(),
            history: History::default(),
            server_cfg: ServerConfigStore::default(),
            user_cfg: UserConfigStore::default(),
            conversations: Mutex::new(ConversationTracker::new(idle)),
            processing: Mutex::new(HashSet::new()),
            responded: Mutex::new(VecDeque::with_capacity(200)),
            paginated: Mutex::new(HashMap::new()),
            reminder_started: AtomicBool::new(false),
        }
    }

    async fn already_seen(&self, id: u64) -> bool {
        let mut processing = self.processing.lock().await;
        let responded = self.responded.lock().await;
        if processing.contains(&id) || responded.contains(&id) {
            return true;
        }
        processing.insert(id);
        false
    }

    async fn mark_done(&self, id: u64) {
        self.processing.lock().await.remove(&id);
        let mut responded = self.responded.lock().await;
        if responded.len() >= 200 {
            responded.pop_front();
        }
        responded.push_back(id);
    }

    async fn handle_new(&self, channel_id: u64, user_id: u64) -> String {
        self.agent.reset_session(&user_id.to_string()).await;
        self.conversations.lock().await.remove(channel_id, user_id);
        "New conversation started. Your previous conversation history has been cleared.".into()
    }

    async fn handle_reset(&self, channel_id: u64, user_id: u64) -> String {
        self.handle_new(channel_id, user_id).await
    }

    async fn respond(&self, ctx: &Context, msg: &Message, content: &str) {
        let _ = reply_no_ping(ctx, msg, content).await;
    }
}

/// Handle a `/config` interaction, returning the reply text (sent ephemerally).
async fn handle_config_interaction(
    server_cfg: &ServerConfigStore,
    user_cfg: &UserConfigStore,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
    guild_id: Option<u64>,
) -> String {
    let Some(top) = options.first() else {
        return "No subcommand provided.".into();
    };

    match top.name.as_str() {
        "channel" => {
            let Some(gid) = guild_id else {
                return "Channel configuration is only available in servers, not DMs.".into();
            };
            let sub_opts = match &top.value {
                CommandDataOptionValue::SubCommandGroup(opts) => opts,
                _ => return "Unexpected option structure.".into(),
            };
            let Some(sub) = sub_opts.first() else {
                return "No channel subcommand provided.".into();
            };
            match sub.name.as_str() {
                "list" => {
                    let cfg = server_cfg.load(gid).await;
                    if cfg.allowed_channel_ids.is_empty() {
                        "I'm allowed to respond in **all channels** (no restriction set).".into()
                    } else {
                        let ids: Vec<String> = cfg
                            .allowed_channel_ids
                            .iter()
                            .map(|id| format!("<#{id}>"))
                            .collect();
                        format!("Allowed channels: {}", ids.join(", "))
                    }
                }
                "clear" => {
                    let mut cfg = server_cfg.load(gid).await;
                    cfg.allowed_channel_ids.clear();
                    if server_cfg.save(gid, &cfg).await.is_err() {
                        return "Error: failed to save config.".into();
                    }
                    "✅ Channel restriction cleared — I'll respond in all channels.".into()
                }
                action @ ("add" | "remove") => {
                    let channel_opts = match &sub.value {
                        CommandDataOptionValue::SubCommand(opts) => opts,
                        _ => return "Unexpected option structure.".into(),
                    };
                    let channel_id =
                        channel_opts
                            .iter()
                            .find(|o| o.name == "channel")
                            .and_then(|o| match &o.value {
                                CommandDataOptionValue::Channel(c) => Some(c.get()),
                                _ => None,
                            });
                    let Some(cid) = channel_id else {
                        return "Please provide a valid channel.".into();
                    };
                    let mut cfg = server_cfg.load(gid).await;
                    if action == "add" {
                        cfg.allowed_channel_ids.insert(cid);
                        if server_cfg.save(gid, &cfg).await.is_err() {
                            return "Error: failed to save config.".into();
                        }
                        format!("✅ <#{cid}> added to the allowlist.")
                    } else {
                        let removed = cfg.allowed_channel_ids.remove(&cid);
                        if server_cfg.save(gid, &cfg).await.is_err() {
                            return "Error: failed to save config.".into();
                        }
                        if removed {
                            format!("✅ <#{cid}> removed from the allowlist.")
                        } else {
                            format!("<#{cid}> was not in the allowlist.")
                        }
                    }
                }
                other => format!("Unknown channel subcommand `{other}`."),
            }
        }

        "personality" => {
            let sub_opts = match &top.value {
                CommandDataOptionValue::SubCommand(opts) => opts,
                _ => return "Unexpected option structure.".into(),
            };
            let text = sub_opts
                .iter()
                .find(|o| o.name == "text")
                .and_then(|o| match &o.value {
                    CommandDataOptionValue::String(s) => Some(s.clone()),
                    _ => None,
                });
            let mut cfg = user_cfg.load(author_id).await;
            match text {
                None => {
                    cfg.personality = None;
                    if user_cfg.save(author_id, &cfg).await.is_err() {
                        return "Error: failed to save config.".into();
                    }
                    "✅ Personality cleared — I'll use my default behaviour.".into()
                }
                Some(ref s) if s.trim().is_empty() => {
                    cfg.personality = None;
                    if user_cfg.save(author_id, &cfg).await.is_err() {
                        return "Error: failed to save config.".into();
                    }
                    "✅ Personality cleared — I'll use my default behaviour.".into()
                }
                Some(s) => {
                    cfg.personality = Some(s.clone());
                    if user_cfg.save(author_id, &cfg).await.is_err() {
                        return "Error: failed to save config.".into();
                    }
                    format!("✅ Personality set:\n> {}", s.replace('\n', "\n> "))
                }
            }
        }

        "followup" => {
            let sub_opts = match &top.value {
                CommandDataOptionValue::SubCommand(opts) => opts,
                _ => return "Unexpected option structure.".into(),
            };
            let enabled =
                sub_opts
                    .iter()
                    .find(|o| o.name == "enabled")
                    .and_then(|o| match &o.value {
                        CommandDataOptionValue::Boolean(b) => Some(*b),
                        _ => None,
                    });
            let timeout =
                sub_opts
                    .iter()
                    .find(|o| o.name == "timeout")
                    .and_then(|o| match &o.value {
                        CommandDataOptionValue::Integer(n) => Some(*n),
                        _ => None,
                    });
            let Some(enabled) = enabled else {
                return "Please specify `enabled`.".into();
            };
            let mut cfg = user_cfg.load(author_id).await;
            cfg.followup_enabled = enabled;
            if let Some(secs) = timeout {
                if secs < 1 {
                    return "Timeout must be at least 1 second.".into();
                }
                cfg.followup_timeout_secs = secs as u64;
            }
            if user_cfg.save(author_id, &cfg).await.is_err() {
                return "Error: failed to save config.".into();
            }
            let status = if enabled { "enabled" } else { "disabled" };
            format!(
                "✅ Follow-up replies {status} (timeout: {}s).",
                cfg.followup_timeout_secs
            )
        }

        other => format!("Unknown config option `{other}`."),
    }
}

async fn handle_labs_interaction(
    user_cfg: &UserConfigStore,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
) -> String {
    let mut cfg = user_cfg.load(author_id).await;
    let Some(top) = options.first() else {
        return "Choose a labs feature. Use `/labs list` to see available features.".into();
    };
    match top.name.as_str() {
        "list" => format!(
            "**Labs features**\n• Pagination: {}",
            if cfg.labs_pagination_enabled {
                "enabled"
            } else {
                "disabled"
            }
        ),
        "pagination" => {
            let CommandDataOptionValue::SubCommand(sub_opts) = &top.value else {
                return "Unexpected option structure.".into();
            };
            let Some(enabled) =
                sub_opts
                    .iter()
                    .find(|o| o.name == "enabled")
                    .and_then(|o| match &o.value {
                        CommandDataOptionValue::Boolean(value) => Some(*value),
                        _ => None,
                    })
            else {
                return "Please specify `enabled`.".into();
            };
            cfg.labs_pagination_enabled = enabled;
            if let Err(error) = user_cfg.save(author_id, &cfg).await {
                tracing::error!(target: "housebot::labs::pagination", user_id = author_id, %error, "Failed to save pagination setting");
                return "Error: failed to save labs configuration.".into();
            }
            tracing::info!(target: "housebot::labs::pagination", user_id = author_id, enabled, "Updated pagination setting");
            format!(
                "✅ Paginated responses {}.",
                if enabled { "enabled" } else { "disabled" }
            )
        }
        other => format!("Unknown labs feature `{other}`. Use `/labs list`."),
    }
}

async fn reply_no_ping(ctx: &Context, msg: &Message, content: &str) -> serenity::Result<Message> {
    let builder = CreateMessage::new()
        .content(content)
        .reference_message(msg)
        .allowed_mentions(CreateAllowedMentions::new());
    msg.channel_id.send_message(&ctx.http, builder).await
}

#[serenity::async_trait]
impl EventHandler for HouseBot {
    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!("Logged in as {} (ID: {})", ready.user.name, ready.user.id);

        // Register the /config global slash command.
        let config_cmd = CreateCommand::new("config")
            .description("Configure bot settings")
            // ── channel subcommand group ─────────────────────────────────────
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommandGroup,
                    "channel",
                    "Manage which channels the bot responds in (server-wide)",
                )
                .add_sub_option(CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "list",
                    "Show the current channel allowlist",
                ))
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::SubCommand,
                        "add",
                        "Add a channel to the allowlist",
                    )
                    .add_sub_option(
                        CreateCommandOption::new(
                            CommandOptionType::Channel,
                            "channel",
                            "The channel to allow",
                        )
                        .required(true),
                    ),
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::SubCommand,
                        "remove",
                        "Remove a channel from the allowlist",
                    )
                    .add_sub_option(
                        CreateCommandOption::new(
                            CommandOptionType::Channel,
                            "channel",
                            "The channel to remove",
                        )
                        .required(true),
                    ),
                )
                .add_sub_option(CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "clear",
                    "Remove all channel restrictions (bot responds everywhere)",
                )),
            )
            // ── personality subcommand ───────────────────────────────────────
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "personality",
                    "Set or clear your personal bot personality / tone override",
                )
                .add_sub_option(CreateCommandOption::new(
                    CommandOptionType::String,
                    "text",
                    "Personality description (omit to clear your override)",
                )),
            )
            // ── followup subcommand ──────────────────────────────────────────
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "followup",
                    "Control whether the bot replies without a ping during active conversations",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::Boolean,
                        "enabled",
                        "Enable or disable follow-up replies",
                    )
                    .required(true),
                )
                .add_sub_option(CreateCommandOption::new(
                    CommandOptionType::Integer,
                    "timeout",
                    "Seconds to keep the conversation open without a ping (default 300)",
                )),
            );

        if let Err(e) = Command::create_global_command(&ctx.http, config_cmd).await {
            tracing::error!("Failed to register /config slash command: {e}");
        }
        let labs_cmd = CreateCommand::new("labs")
            .description("Enable experimental bot features")
            .add_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "list",
                "List experimental features and their status",
            ))
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "pagination",
                    "Toggle paginated LLM responses",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::Boolean,
                        "enabled",
                        "Enable or disable paginated responses",
                    )
                    .required(true),
                ),
            );
        if let Err(e) = Command::create_global_command(&ctx.http, labs_cmd).await {
            tracing::error!(target: "housebot::labs::registration", "Failed to register /labs slash command: {e}");
        }
        for command in [
            CreateCommand::new("model").description("Show information about the current model"),
            CreateCommand::new("session")
                .description("Show context and token usage for this session"),
            CreateCommand::new("new").description("Start a new conversation and clear the old one"),
            CreateCommand::new("reset").description("Clear the conversation and start fresh"),
            CreateCommand::new("compact")
                .description("Summarize the conversation and start a new session"),
        ] {
            if let Err(e) = Command::create_global_command(&ctx.http, command).await {
                tracing::error!("Failed to register slash command: {e}");
            }
        }

        if self.reminder_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let http = ctx.http.clone();
        let reminders = self.agent.reminders().clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let now = unix_now();
                for r in reminders.pop_due(now).await {
                    if let Ok(uid) = r.user_id.parse::<u64>() {
                        if let Ok(dm) = UserId::new(uid).create_dm_channel(&http).await {
                            let _ = dm
                                .say(&http, format!("⏰ **Reminder:** {}", r.message))
                                .await;
                        }
                    }
                }
            }
        });
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Component(component) = &interaction {
            self.handle_pagination_component(&ctx, component).await;
            return;
        }
        let Interaction::Command(cmd) = interaction else {
            return;
        };
        let user_id = cmd.user.id.get();
        let guild_id = cmd.guild_id.map(|g| g.get());
        if cmd.data.name == "compact" {
            let response = CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(true),
            );
            if let Err(e) = cmd.create_response(&ctx.http, response).await {
                tracing::warn!("Failed to defer /compact response: {e}");
                return;
            }
            let hooks = CompactProgressHooks(CompactProgressTarget::Interaction {
                ctx: ctx.clone(),
                command: Box::new(cmd.clone()),
            });
            self.agent
                .compact_session_with_hooks(&user_id.to_string(), &hooks)
                .await;
            self.conversations
                .lock()
                .await
                .remove(cmd.channel_id.get(), user_id);
            return;
        }
        let reply = match cmd.data.name.as_str() {
            "config" => {
                handle_config_interaction(
                    &self.server_cfg,
                    &self.user_cfg,
                    &cmd.data.options,
                    user_id,
                    guild_id,
                )
                .await
            }
            "labs" => handle_labs_interaction(&self.user_cfg, &cmd.data.options, user_id).await,
            "model" => self.agent.model_info(),
            "session" => {
                let info = self.agent.session_info(&user_id.to_string()).await;
                let percent =
                    info.context_tokens as f64 / info.context_window_tokens.max(1) as f64 * 100.0;
                let response = CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .embed(
                            CreateEmbed::new()
                                .title("Session")
                                .field(
                                    "Context",
                                    format!(
                                        "{} / {} tokens ({percent:.1}%)",
                                        info.context_tokens, info.context_window_tokens
                                    ),
                                    true,
                                )
                                .field("Messages", info.messages.to_string(), true)
                                .field("Model requests", info.requests.to_string(), true)
                                .field("Input tokens", info.input_tokens.to_string(), true)
                                .field("Output tokens", info.output_tokens.to_string(), true)
                                .field("Cached tokens", info.cached_tokens.to_string(), true),
                        )
                        .ephemeral(true),
                );
                if let Err(e) = cmd.create_response(&ctx.http, response).await {
                    tracing::warn!("Failed to send /session response: {e}");
                }
                return;
            }
            "new" => self.handle_new(cmd.channel_id.get(), user_id).await,
            "reset" => self.handle_reset(cmd.channel_id.get(), user_id).await,
            _ => return,
        };

        let response = CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new()
                .content(reply)
                .ephemeral(true),
        );
        if let Err(e) = cmd.create_response(&ctx.http, response).await {
            tracing::warn!("Failed to send /config response: {e}");
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }
        let content = msg.content.trim().to_string();
        let channel_id = msg.channel_id.get();
        let user_id = msg.author.id.get();

        // ── commands ──
        if content == "!reset" {
            let reply = self.handle_reset(channel_id, user_id).await;
            self.respond(&ctx, &msg, &reply).await;
            return;
        }
        if content == "!new" {
            let reply = self.handle_new(channel_id, user_id).await;
            self.respond(&ctx, &msg, &reply).await;
            return;
        }
        if content == "!compact" {
            let progress = reply_no_ping(&ctx, &msg, &compact_progress(0, None))
                .await
                .ok();
            if let Some(progress) = &progress {
                let hooks = CompactProgressHooks(CompactProgressTarget::Message {
                    ctx: ctx.clone(),
                    channel_id: msg.channel_id,
                    message_id: progress.id,
                });
                self.agent
                    .compact_session_with_hooks(&user_id.to_string(), &hooks)
                    .await;
            } else {
                self.agent.compact_session(&user_id.to_string()).await;
            }
            self.conversations.lock().await.remove(channel_id, user_id);
            if let Some(mut progress) = progress {
                let _ = progress
                    .edit(
                        &ctx.http,
                        EditMessage::new().content(
                            "✅ Conversation compacted into memory. A new session has started.",
                        ),
                    )
                    .await;
            } else {
                self.respond(
                    &ctx,
                    &msg,
                    "✅ Conversation compacted into memory. A new session has started.",
                )
                .await;
            }
            return;
        }
        if msg.content.starts_with("!skill") {
            let (first, rest) = split_command(&msg.content);
            let reply = skill_command(&self.skills, &first, &rest, user_id).await;
            self.respond(&ctx, &msg, &reply).await;
            return;
        }
        if msg.content.starts_with("!note") {
            let (first, rest) = split_command(&msg.content);
            let reply = note_command(&self.notes, &first, &rest, user_id).await;
            self.respond(&ctx, &msg, &reply).await;
            return;
        }
        if content == "!stats" {
            let reply = stats_command(
                &self.history,
                &self.memory,
                &self.notes,
                &self.skills,
                user_id,
                &msg.author.name,
            )
            .await;
            self.respond(&ctx, &msg, &reply).await;
            return;
        }
        // ── routing ──
        let bot_id = ctx.cache.current_user().id;
        let is_dm = msg.guild_id.is_none();
        let guild_id = msg.guild_id.map(|g| g.get());

        // Check channel allowlist before doing anything else.
        if !self
            .server_cfg
            .is_channel_allowed(guild_id, channel_id)
            .await
        {
            return;
        }

        let is_mentioned = msg.mentions.iter().any(|u| u.id == bot_id);
        let is_reply_to_bot = msg
            .referenced_message
            .as_ref()
            .map(|m| m.author.id == bot_id)
            .unwrap_or(false);

        // Load per-user followup settings.
        let user_config = self.user_cfg.load(user_id).await;
        let followup_enabled = user_config.followup_enabled;
        let followup_timeout = Duration::from_secs(user_config.followup_timeout_secs);

        let now = Instant::now();
        let (is_active, session_expired) = {
            let mut convos = self.conversations.lock().await;
            let active = followup_enabled && convos.is_active(channel_id, user_id, now);
            let expired = !active && convos.pop_timed_out(channel_id, user_id, now);
            (active, expired)
        };

        if !(is_dm || is_mentioned || is_reply_to_bot || is_active) {
            return;
        }
        if self.already_seen(msg.id.get()).await {
            tracing::warn!("Duplicate message {} — skipping", msg.id.get());
            return;
        }

        self.handle_message(&ctx, &msg, bot_id, session_expired, followup_timeout)
            .await;
        self.mark_done(msg.id.get()).await;
    }
}

impl HouseBot {
    async fn handle_message(
        &self,
        ctx: &Context,
        msg: &Message,
        bot_id: UserId,
        session_expired: bool,
        followup_timeout: Duration,
    ) {
        let mut text = msg.content.clone();
        for token in [format!("<@{bot_id}>"), format!("<@!{bot_id}>")] {
            text = text.replace(&token, "");
        }
        let text = text.trim().to_string();

        let referenced_text = msg
            .referenced_message
            .as_deref()
            .and_then(referenced_message_context);
        let text = match referenced_text {
            Some(referenced) if text.is_empty() => referenced,
            Some(referenced) => format!("{text}\n\n{referenced}"),
            None => text,
        };
        if text.is_empty() && msg.attachments.is_empty() {
            return;
        }

        if session_expired {
            self.agent
                .compact_session(&msg.author.id.get().to_string())
                .await;
        }

        let mut images = extract_images(msg).await;
        if let Some(referenced) = msg.referenced_message.as_deref() {
            images.extend(extract_images(referenced).await);
        }

        // Load personality for this user.
        let user_config = self.user_cfg.load(msg.author.id.get()).await;
        let personality = user_config.personality.clone();

        // Keep the progress message in the thinking state until the model starts its final reply.
        let progress = reply_no_ping(ctx, msg, "🧠 **Thinking...**").await.ok();
        let pending_reaction = msg.react(&ctx.http, '⏳').await.ok();

        let response_hooks = progress
            .as_ref()
            .map(|progress| ResponseProgressHooks::new(ctx, progress));

        let user_text = if text.is_empty() {
            "(no text)".to_string()
        } else {
            text
        };
        let result: AgentResult = self
            .agent
            .run(
                &msg.author.id.get().to_string(),
                &msg.author.name,
                &user_text,
                &images,
                response_hooks
                    .as_ref()
                    .map_or(&NoHooks as &dyn AgentHooks, |hooks| {
                        hooks as &dyn AgentHooks
                    }),
                personality.as_deref(),
            )
            .await;

        {
            let mut convos = self.conversations.lock().await;
            convos.mark_active(
                msg.channel_id.get(),
                msg.author.id.get(),
                Instant::now(),
                followup_timeout,
            );
        }

        let safe = self.redactor.redact(&result.text);
        if let Some(notice) = &result.session_notice {
            let _ = reply_no_ping(ctx, msg, notice).await;
        }
        let with_tool_summary = append_tool_summary(&safe, &result.tools_called);
        let (display, code_files) = extract_code_files(&with_tool_summary);
        send_final_message(
            ctx,
            msg,
            &display,
            user_config.labs_pagination_enabled,
            msg.author.id.get(),
            &self.paginated,
            progress.as_ref(),
        )
        .await;

        if let Some(reaction) = pending_reaction {
            let _ = reaction.delete(&ctx.http).await;
        }
        // Upload extracted code blocks.
        for (filename, content) in code_files {
            let safe = self.redactor.redact(&String::from_utf8_lossy(&content));
            let _ = msg
                .channel_id
                .send_message(
                    &ctx.http,
                    CreateMessage::new()
                        .add_file(CreateAttachment::bytes(safe.into_bytes(), filename)),
                )
                .await;
        }

        // Upload sandbox artifacts (strip the uid_ prefix, redact contents).
        for path in &result.artifact_paths {
            if let Ok(raw) = tokio::fs::read(path).await {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "artifact".into());
                let display_name = name
                    .split_once('_')
                    .map(|(_, r)| r.to_string())
                    .unwrap_or(name);
                let safe = self.redactor.redact(&String::from_utf8_lossy(&raw));
                let _ = msg
                    .channel_id
                    .send_message(
                        &ctx.http,
                        CreateMessage::new()
                            .add_file(CreateAttachment::bytes(safe.into_bytes(), display_name)),
                    )
                    .await;
            }
        }
    }

    async fn handle_pagination_component(
        &self,
        ctx: &Context,
        component: &serenity::all::ComponentInteraction,
    ) {
        let Some(rest) = component.data.custom_id.strip_prefix(PAGINATION_PREFIX) else {
            return;
        };
        let Some((token, page)) = rest.rsplit_once(':') else {
            return;
        };
        let Ok(page) = page.parse::<usize>() else {
            return;
        };
        let response = self
            .paginated
            .lock()
            .await
            .get(token)
            .map(|response| (response.owner_id, response.pages.clone()));
        let Some((owner_id, pages)) = response else {
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("This paginated response has expired.")
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        };
        if owner_id != component.user.id.get() || page >= pages.len() {
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("Only the response author can use these buttons.")
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        }
        let response = CreateInteractionResponse::UpdateMessage(
            CreateInteractionResponseMessage::new()
                .embed(pagination_embed(&pages, page))
                .components(pagination_components(token, page, pages.len())),
        );
        let _ = component.create_response(&ctx.http, response).await;
    }
}

fn split_command(content: &str) -> (String, String) {
    match content.split_once('\n') {
        Some((first, rest)) => (first.trim().to_string(), rest.trim().to_string()),
        None => (content.trim().to_string(), String::new()),
    }
}

async fn send_final_message(
    ctx: &Context,
    msg: &Message,
    text: &str,
    paginate: bool,
    owner_id: u64,
    store: &Mutex<HashMap<String, PaginatedResponse>>,
    progress: Option<&Message>,
) {
    if !paginate {
        let chunks = split_text(text, MAX_MESSAGE_LENGTH);
        if let (Some(progress), Some(first)) = (progress, chunks.first()) {
            if progress
                .channel_id
                .edit_message(
                    &ctx.http,
                    progress.id,
                    EditMessage::new()
                        .content(first)
                        .allowed_mentions(CreateAllowedMentions::new()),
                )
                .await
                .is_ok()
            {
                for chunk in chunks.iter().skip(1) {
                    let _ = msg.channel_id.say(&ctx.http, chunk).await;
                }
                return;
            }
        }
        for (i, chunk) in chunks.iter().enumerate() {
            if i == 0 {
                let _ = reply_no_ping(ctx, msg, chunk).await;
            } else {
                let _ = msg.channel_id.say(&ctx.http, chunk).await;
            }
        }
        return;
    }

    if let Some(progress) = progress {
        let _ = progress.delete(&ctx.http).await;
    }
    let pages = split_text(text, EMBED_DESCRIPTION_LIMIT);
    let token = uuid::Uuid::new_v4().simple().to_string();
    store.lock().await.insert(
        token.clone(),
        PaginatedResponse {
            owner_id,
            pages: pages.clone(),
        },
    );
    let builder = CreateMessage::new()
        .embed(pagination_embed(&pages, 0))
        .components(pagination_components(&token, 0, pages.len()))
        .reference_message(msg)
        .allowed_mentions(CreateAllowedMentions::new());
    let _ = msg.channel_id.send_message(&ctx.http, builder).await;
}

fn pagination_embed(pages: &[String], page: usize) -> CreateEmbed {
    CreateEmbed::new()
        .description(&pages[page])
        .footer(serenity::all::CreateEmbedFooter::new(format!(
            "Page {} of {}",
            page + 1,
            pages.len()
        )))
}

fn pagination_components(token: &str, page: usize, page_count: usize) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!(
            "{PAGINATION_PREFIX}{token}:{}",
            page.saturating_sub(1)
        ))
        .label("←")
        .style(ButtonStyle::Secondary)
        .disabled(page == 0),
        CreateButton::new(format!("{PAGINATION_PREFIX}{token}:{}", page + 1))
            .label("→")
            .style(ButtonStyle::Secondary)
            .disabled(page + 1 >= page_count),
    ])]
}

fn append_tool_summary(text: &str, tools: &[String]) -> String {
    let summary = if tools.is_empty() {
        "none".to_string()
    } else {
        tools
            .iter()
            .map(|tool| format!("`{tool}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("{text}\n\n🛠️ **Tools used:** {summary}")
}

async fn extract_images(msg: &Message) -> Vec<ImageData> {
    let mut images = Vec::new();
    for att in &msg.attachments {
        let Some(media_type) = image_media_type(&att.filename) else {
            continue;
        };
        if let Ok(resp) = reqwest::get(&att.url).await {
            if let Ok(bytes) = resp.bytes().await {
                use base64::Engine;
                images.push(ImageData {
                    media_type: media_type.to_string(),
                    data: base64::engine::general_purpose::STANDARD.encode(&bytes),
                });
            }
        }
    }
    images
}

fn image_media_type(filename: &str) -> Option<&'static str> {
    match filename
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        _ => None,
    }
}

fn referenced_message_context(msg: &Message) -> Option<String> {
    let text = msg.content.trim();
    let urls: Vec<&str> = URL.find_iter(text).map(|m| m.as_str()).collect();
    let has_images = msg
        .attachments
        .iter()
        .any(|attachment| image_media_type(&attachment.filename).is_some());
    if text.is_empty() && !has_images {
        return None;
    }

    let mut context = String::from("[Message being replied to]\n");
    if !text.is_empty() {
        context.push_str(text);
    }
    if !urls.is_empty() {
        context.push_str(
            "\n\nThe message above contains URL(s). Use the web fetch tool on these URL(s) before answering: ",
        );
        context.push_str(&urls.join(", "));
    }
    if has_images {
        context.push_str("\n\n[The message above also contains image attachment(s).]");
    }
    context.push_str("\n[End message being replied to]");
    Some(context)
}

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Run the bot: build the agent, register the handler, and connect to Discord.
pub async fn run() -> anyhow::Result<()> {
    let token = std::env::var("DISCORD_BOT_TOKEN")
        .map_err(|_| anyhow::anyhow!("DISCORD_BOT_TOKEN is not set"))?;
    let agent = Arc::new(Agent::from_env().await);
    let bot = HouseBot::new(agent);

    let intents = GatewayIntents::non_privileged() | GatewayIntents::MESSAGE_CONTENT;
    let mut client = Client::builder(&token, intents).event_handler(bot).await?;
    tracing::info!("Agent and MCP servers ready");
    client.start().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    // ── split_text ──
    #[test]
    fn split_short_text_single_chunk() {
        assert_eq!(split_text("hello", 2000), vec!["hello"]);
    }

    #[test]
    fn split_exact_limit_not_split() {
        let text = "a".repeat(2000);
        assert_eq!(split_text(&text, 2000), vec![text.clone()]);
    }

    #[test]
    fn split_over_limit_on_newline() {
        let text = format!("{}\n{}", "a".repeat(1900), "b".repeat(200));
        let chunks = split_text(&text, 2000);
        assert_eq!(chunks.len(), 2);
        assert!(chunks.iter().all(|c| c.chars().count() <= 2000));
        assert_eq!(chunks.concat(), text.replacen('\n', "", 1));
    }

    #[test]
    fn split_over_limit_no_newline() {
        let text = "x".repeat(2500);
        let chunks = split_text(&text, 2000);
        assert_eq!(chunks, vec!["x".repeat(2000), "x".repeat(500)]);
    }

    #[test]
    fn split_multiple_chunks() {
        let text = vec!["a".repeat(1999); 3].join("\n");
        let chunks = split_text(&text, 2000);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= 2000));
    }

    #[test]
    fn split_empty_string() {
        assert_eq!(split_text("", 2000), vec![""]);
    }

    #[test]
    fn split_custom_limit() {
        let chunks = split_text("hello\nworld", 6);
        assert_eq!(chunks, vec!["hello", "world"]);
    }

    // ── tool_hint ──
    #[test]
    fn hint_run_skill_with_name_and_input() {
        let h = tool_hint(
            "run_skill",
            &json!({"name": "summarize", "input": "some text"}),
        );
        assert!(h.contains("summarize"));
        assert!(h.contains("some text"));
    }

    #[test]
    fn hint_run_skill_no_name() {
        assert_eq!(tool_hint("run_skill", &json!({"input": "some text"})), "");
    }

    #[test]
    fn hint_falls_back_to_query() {
        assert!(tool_hint("ddg__search", &json!({"query": "latest news"})).contains("latest news"));
    }

    #[test]
    fn hint_falls_back_to_task() {
        assert!(
            tool_hint("run_opencode", &json!({"task": "write a script"}))
                .contains("write a script")
        );
    }

    #[test]
    fn hint_long_value_truncated() {
        let h = tool_hint("run_opencode", &json!({"task": "x".repeat(200)}));
        assert!(h.chars().count() <= 85);
    }

    #[test]
    fn hint_unknown_tool_no_known_key() {
        assert_eq!(tool_hint("some_tool", &json!({"foo": "bar"})), "");
    }

    #[test]
    fn hint_multiline_flattened() {
        let h = tool_hint("run_opencode", &json!({"task": "line1\nline2"}));
        assert!(!h.contains('\n'));
    }

    #[test]
    fn tool_summary_lists_tools_in_call_order() {
        let summary = append_tool_summary("answer", &["ddg__search".into(), "translate".into()]);
        assert!(summary.ends_with("🛠️ **Tools used:** `ddg__search`, `translate`"));
    }

    #[test]
    fn tool_summary_shows_none_when_no_tools_were_called() {
        assert!(append_tool_summary("answer", &[]).ends_with("🛠️ **Tools used:** none"));
    }

    // ── extract_code_files ──
    #[test]
    fn code_short_block_not_extracted() {
        let text = "Here:\n```python\nprint('hi')\n```";
        let (modified, files) = extract_code_files(text);
        assert!(files.is_empty());
        assert!(modified.contains("```"));
    }

    #[test]
    fn code_large_block_extracted() {
        let code = "x = 1\n".repeat(200);
        let text = format!("Here:\n```python\n{code}```");
        let (modified, files) = extract_code_files(&text);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "script_1.py");
        assert_eq!(files[0].1, code.as_bytes());
        assert!(!modified.contains("```"));
        assert!(modified.contains("script_1.py"));
    }

    #[test]
    fn code_extension_from_language() {
        let code = "echo hi\n".repeat(150);
        let (_, files) = extract_code_files(&format!("```bash\n{code}```"));
        assert!(files[0].0.ends_with(".sh"));
    }

    #[test]
    fn code_unknown_language_txt() {
        let code = "blah\n".repeat(200);
        let (_, files) = extract_code_files(&format!("```brainfuck\n{code}```"));
        assert!(files[0].0.ends_with(".txt"));
    }

    #[test]
    fn code_unclosed_block_still_extracted() {
        let code = "x = 1\n".repeat(200);
        let (modified, files) = extract_code_files(&format!("```python\n{code}"));
        assert_eq!(files.len(), 1);
        assert!(modified.contains("script_1.py"));
    }

    #[test]
    fn code_multiple_blocks_numbered() {
        let code = "x = 1\n".repeat(200);
        let (_, files) = extract_code_files(&format!("```python\n{code}```\n```bash\n{code}```"));
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "script_1.py");
        assert_eq!(files[1].0, "script_2.sh");
    }

    #[test]
    fn code_mixed_small_and_large() {
        let small = "print('hi')\n";
        let large = "x = 1\n".repeat(200);
        let (modified, files) =
            extract_code_files(&format!("```python\n{small}```\n```python\n{large}```"));
        assert_eq!(files.len(), 1);
        assert!(modified.contains("script_1.py"));
        assert!(modified.contains("```python"));
    }

    // ── redaction ──
    #[test]
    fn redact_known_secret() {
        let r = SecretRedactor::from_vars([(
            "MY_SECRET_TOKEN".into(),
            "super-secret-token-abc123xyz".into(),
        )]);
        let out = r.redact("The token is super-secret-token-abc123xyz");
        assert!(!out.contains("super-secret-token-abc123xyz"));
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn redact_non_secret_env_not_redacted() {
        let r = SecretRedactor::from_vars([("MY_NAME".into(), "alice-longenough".into())]);
        assert_eq!(r.redact("hello alice-longenough"), "hello alice-longenough");
    }

    #[test]
    fn redact_short_value_not_redacted() {
        let r = SecretRedactor::from_vars([("MY_TOKEN".into(), "abc".into())]);
        assert_eq!(r.redact("abc"), "abc");
    }

    #[test]
    fn redact_multiple_secrets() {
        let r = SecretRedactor::from_vars([
            ("BOT_TOKEN".into(), "discord-token-xyz987".into()),
            ("JELLYFIN_API_KEY".into(), "jellyfin-api-key-456def".into()),
        ]);
        let out = r.redact("token=discord-token-xyz987 key=jellyfin-api-key-456def");
        assert!(!out.contains("discord-token-xyz987"));
        assert!(!out.contains("jellyfin-api-key-456def"));
        assert_eq!(out.matches("[REDACTED]").count(), 2);
    }

    #[test]
    fn redact_text_without_secrets_unchanged() {
        let r = SecretRedactor::from_vars(std::iter::empty());
        assert_eq!(
            r.redact("hello world, no secrets here"),
            "hello world, no secrets here"
        );
    }

    // ── conversation tracker ──
    #[test]
    fn tracker_inactive_when_unknown() {
        let t = ConversationTracker::new(Duration::from_secs(300));
        assert!(!t.is_active(1, 2, Instant::now()));
    }

    #[test]
    fn tracker_active_within_window() {
        let mut t = ConversationTracker::new(Duration::from_secs(300));
        let now = Instant::now();
        t.mark_active(1, 2, now, Duration::from_secs(300));
        assert!(t.is_active(1, 2, now + Duration::from_secs(100)));
    }

    #[test]
    fn tracker_pop_timed_out() {
        let mut t = ConversationTracker::new(Duration::from_secs(300));
        let now = Instant::now();
        t.mark_active(1, 2, now, Duration::from_secs(300));
        assert!(!t.is_active(1, 2, now + Duration::from_secs(400)));
        assert!(t.pop_timed_out(1, 2, now + Duration::from_secs(400)));
        // Now removed.
        assert!(!t.pop_timed_out(1, 2, now + Duration::from_secs(400)));
    }

    // ── commands ──
    fn stores() -> (TempDir, Skills, Notes, Memory, History) {
        let tmp = TempDir::new().unwrap();
        (
            TempDir::new().unwrap(),
            Skills::new(tmp.path().join("skills.json")),
            Notes::new(tmp.path().join("notes")),
            Memory::new(tmp.path().join("memories")),
            History::new(tmp.path().join("history"), 30),
        )
    }

    #[tokio::test]
    async fn skill_add_and_list() {
        let (_t, skills, _n, _m, _h) = stores();
        let add = skill_command(&skills, "!skill add greeter", "You greet people", 7).await;
        assert!(add.contains("saved"));
        let list = skill_command(&skills, "!skill list", "", 7).await;
        assert!(list.contains("greeter"));
    }

    #[tokio::test]
    async fn skill_invalid_name_rejected() {
        let (_t, skills, _n, _m, _h) = stores();
        let out = skill_command(&skills, "!skill add Bad-Name", "prompt", 1).await;
        assert!(out.contains("lowercase"));
    }

    #[tokio::test]
    async fn skill_delete_missing() {
        let (_t, skills, _n, _m, _h) = stores();
        assert!(skill_command(&skills, "!skill delete nope", "", 1)
            .await
            .contains("not found"));
    }

    #[tokio::test]
    async fn note_save_get_delete() {
        let (_t, _s, notes, _m, _h) = stores();
        assert!(
            note_command(&notes, "!note save shopping", "milk, eggs", 42)
                .await
                .contains("saved")
        );
        assert!(note_command(&notes, "!note get shopping", "", 42)
            .await
            .contains("milk, eggs"));
        assert!(note_command(&notes, "!note delete shopping", "", 42)
            .await
            .contains("deleted"));
        assert!(note_command(&notes, "!note get shopping", "", 42)
            .await
            .contains("not found"));
    }

    #[tokio::test]
    async fn note_list_empty() {
        let (_t, _s, notes, _m, _h) = stores();
        assert!(note_command(&notes, "!note list", "", 1)
            .await
            .contains("no saved notes"));
    }

    #[tokio::test]
    async fn stats_reports_counts() {
        let (_t, skills, notes, memory, history) = stores();
        memory.save(5.to_string(), "some memory").await.unwrap();
        notes.save(5, "a", "x").await.unwrap();
        let out = stats_command(&history, &memory, &notes, &skills, 5, "Alice").await;
        assert!(out.contains("Stats for Alice"));
        assert!(out.contains("Saved notes: 1"));
    }
}
