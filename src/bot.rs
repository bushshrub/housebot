//! Discord interface (serenity): message routing, `!`-commands, streaming progress
//! updates, secret redaction, and code/artifact file uploads.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use regex::{Captures, Regex};
use serde_json::Value;
use serenity::all::{
    Context, CreateAllowedMentions, CreateAttachment, CreateMessage, EditMessage, EventHandler,
    GatewayIntents, Message, Ready, UserId,
};
use serenity::Client;
use tokio::sync::Mutex;

use crate::agent::{Agent, AgentHooks, AgentResult, ImageData};
use crate::config;
use crate::history::History;
use crate::memory::Memory;
use crate::notes::Notes;
use crate::skills::{Skill, Skills};

const MAX_MESSAGE_LENGTH: usize = 2000;
const CODE_FILE_THRESHOLD: usize = 800;
/// Minimum seconds between edits to the same progress message (Discord rate-limits edits).
const EDIT_INTERVAL: Duration = Duration::from_millis(1200);

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

/// Redacts known secret values (drawn from the environment) from outbound text.
pub struct SecretRedactor {
    secrets: Vec<String>,
}

impl SecretRedactor {
    const KEYWORDS: &'static [&'static str] = &[
        "token", "key", "secret", "password", "dsn", "api_key", "oauth",
    ];

    /// Build from the process environment.
    pub fn from_env() -> Self {
        Self::from_vars(std::env::vars())
    }

    /// Build from an explicit iterator of `(name, value)` pairs.
    pub fn from_vars(vars: impl IntoIterator<Item = (String, String)>) -> Self {
        let secrets = vars
            .into_iter()
            .filter(|(name, value)| {
                value.len() >= 8
                    && Self::KEYWORDS
                        .iter()
                        .any(|kw| name.to_lowercase().contains(kw))
            })
            .map(|(_, value)| value)
            .collect();
        Self { secrets }
    }

    /// Replace every known secret value with `[REDACTED]`.
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), "[REDACTED]");
            }
        }
        out
    }
}

/// Tracks which (channel, user) conversations are still within the idle window.
pub struct ConversationTracker {
    idle_timeout: Duration,
    last_active: std::collections::HashMap<(u64, u64), Instant>,
}

impl ConversationTracker {
    pub fn new(idle_timeout: Duration) -> Self {
        Self {
            idle_timeout,
            last_active: std::collections::HashMap::new(),
        }
    }

    pub fn is_active(&self, channel_id: u64, user_id: u64, now: Instant) -> bool {
        match self.last_active.get(&(channel_id, user_id)) {
            Some(&t) => now.duration_since(t) <= self.idle_timeout,
            None => false,
        }
    }

    /// Remove an expired entry; return whether one existed.
    pub fn pop_timed_out(&mut self, channel_id: u64, user_id: u64, now: Instant) -> bool {
        let key = (channel_id, user_id);
        if let Some(&t) = self.last_active.get(&key) {
            if now.duration_since(t) > self.idle_timeout {
                self.last_active.remove(&key);
                return true;
            }
        }
        false
    }

    pub fn mark_active(&mut self, channel_id: u64, user_id: u64, now: Instant) {
        self.last_active.insert((channel_id, user_id), now);
    }

    pub fn remove(&mut self, channel_id: u64, user_id: u64) {
        self.last_active.remove(&(channel_id, user_id));
    }
}

// ── command handlers (store-backed, Discord-independent) ─────────────────────

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Handle a `!skill ...` command, returning the reply text.
pub async fn skill_command(
    skills: &Skills,
    first_line: &str,
    rest: &str,
    author_id: u64,
) -> String {
    let parts: Vec<&str> = first_line
        .splitn(3, char::is_whitespace)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() < 2 {
        return "Usage: `!skill list` | `!skill add <name>` | `!skill delete <name>` | `!skill info <name>`".into();
    }
    match parts[1].to_lowercase().as_str() {
        "list" => {
            let all = skills.load_all().await;
            if all.is_empty() {
                return "No skills defined yet. Use `!skill add <name>` (with the prompt on the next line).".into();
            }
            let mut lines = vec!["**Skills:**".to_string()];
            for s in all.values() {
                lines.push(format!(
                    "• **{}** — {}",
                    s.name,
                    truncate_chars(s.description_or_name(), 80)
                ));
            }
            lines.join("\n")
        }
        "info" => {
            let Some(name) = parts.get(2).map(|s| s.to_lowercase()) else {
                return "Usage: `!skill info <name>`".into();
            };
            match skills.get(&name).await {
                None => format!("Skill `{name}` not found."),
                Some(s) => {
                    let mut preview = truncate_chars(&s.prompt, 500);
                    if s.prompt.chars().count() > 500 {
                        preview.push('…');
                    }
                    format!(
                        "**Skill: {}**\nDescription: {}\n```\n{}\n```",
                        s.name,
                        s.description.as_deref().unwrap_or("(none)"),
                        preview
                    )
                }
            }
        }
        "add" => {
            let Some(name) = parts.get(2).map(|s| s.trim().to_lowercase()) else {
                return "Usage: `!skill add <name>` with the skill prompt on the next line.".into();
            };
            if !valid_name(&name) {
                return "Skill name must be lowercase letters, numbers, and underscores only."
                    .into();
            }
            if rest.is_empty() {
                return "Please include the skill prompt on a new line after the command.".into();
            }
            let description = if rest.chars().count() > 100 {
                format!("{}…", truncate_chars(rest, 100))
            } else {
                rest.to_string()
            };
            let skill = Skill {
                name: name.clone(),
                description: Some(description),
                prompt: rest.to_string(),
                created_by: Some(author_id.to_string()),
            };
            if skills.save(skill).await.is_err() {
                return "Error: failed to save skill.".into();
            }
            format!("✅ Skill **{name}** saved.")
        }
        "delete" => {
            let Some(name) = parts.get(2).map(|s| s.to_lowercase()) else {
                return "Usage: `!skill delete <name>`".into();
            };
            match skills.delete(&name).await {
                Ok(true) => format!("✅ Skill **{name}** deleted."),
                _ => format!("Skill `{name}` not found."),
            }
        }
        other => format!("Unknown subcommand `{other}`. Options: `list`, `add`, `delete`, `info`"),
    }
}

/// Handle a `!note ...` command, returning the reply text.
pub async fn note_command(notes: &Notes, first_line: &str, rest: &str, author_id: u64) -> String {
    let parts: Vec<&str> = first_line
        .splitn(3, char::is_whitespace)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() < 2 {
        return "Usage: `!note list` | `!note save <name>` | `!note get <name>` | `!note delete <name>`".into();
    }
    match parts[1].to_lowercase().as_str() {
        "list" => {
            let all = notes.load_all(author_id).await;
            if all.is_empty() {
                return "You have no saved notes. Use `!note save <name>` (with the content on the next line).".into();
            }
            let mut lines = vec!["**Your notes:**".to_string()];
            for (name, body) in &all {
                let mut preview = truncate_chars(&body.replace('\n', " "), 60);
                if body.chars().count() > 60 {
                    preview.push('…');
                }
                lines.push(format!("• **{name}** — {preview}"));
            }
            lines.join("\n")
        }
        "get" => {
            let Some(name) = parts.get(2).map(|s| s.to_lowercase()) else {
                return "Usage: `!note get <name>`".into();
            };
            match notes.get(author_id, &name).await {
                None => format!("Note `{name}` not found."),
                Some(body) => format!("**{name}:**\n{body}"),
            }
        }
        "save" => {
            let Some(name) = parts.get(2).map(|s| s.trim().to_lowercase()) else {
                return "Usage: `!note save <name>` with the note content on the next line.".into();
            };
            if !valid_name(&name) {
                return "Note name must be lowercase letters, numbers, and underscores only."
                    .into();
            }
            if rest.is_empty() {
                return "Please include the note content on a new line after the command.".into();
            }
            if notes.save(author_id, &name, rest).await.is_err() {
                return "Error: failed to save note.".into();
            }
            format!("✅ Note **{name}** saved.")
        }
        "delete" => {
            let Some(name) = parts.get(2).map(|s| s.to_lowercase()) else {
                return "Usage: `!note delete <name>`".into();
            };
            match notes.delete(author_id, &name).await {
                Ok(true) => format!("✅ Note **{name}** deleted."),
                _ => format!("Note `{name}` not found."),
            }
        }
        other => format!("Unknown subcommand `{other}`. Options: `list`, `save`, `get`, `delete`"),
    }
}

/// Handle a `!stats` command, returning the reply text.
pub async fn stats_command(
    history: &History,
    memory: &Memory,
    notes: &Notes,
    skills: &Skills,
    user_id: u64,
    display_name: &str,
) -> String {
    let hist = history.load(user_id.to_string()).await;
    let mem = memory.load(user_id.to_string()).await;
    let user_notes = notes.load_all(user_id).await;
    let all_skills = skills.load_all().await;

    let turn_count = hist
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        .count();
    let mem_kb = mem.len() as f64 / 1024.0;

    format!(
        "**Stats for {display_name}:**\n• Conversation history: {} messages ({turn_count} turns)\n\
         • Memory size: {mem_kb:.1} KB\n• Saved notes: {}\n• Skills available: {}",
        hist.len(),
        user_notes.len(),
        all_skills.len(),
    )
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
    conversations: Mutex<ConversationTracker>,
    processing: Mutex<HashSet<u64>>,
    responded: Mutex<VecDeque<u64>>,
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
            conversations: Mutex::new(ConversationTracker::new(idle)),
            processing: Mutex::new(HashSet::new()),
            responded: Mutex::new(VecDeque::with_capacity(200)),
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

    async fn handle_reset(&self, channel_id: u64, user_id: u64) -> String {
        self.agent.start_new_session(&user_id.to_string()).await;
        self.conversations.lock().await.remove(channel_id, user_id);
        "Session reset. Your conversation history has been cleared (and summarized if there was any).".into()
    }

    async fn respond(&self, ctx: &Context, msg: &Message, content: &str) {
        let _ = reply_no_ping(ctx, msg, content).await;
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
        let is_mentioned = msg.mentions.iter().any(|u| u.id == bot_id);
        let is_reply_to_bot = msg
            .referenced_message
            .as_ref()
            .map(|m| m.author.id == bot_id)
            .unwrap_or(false);

        let now = Instant::now();
        let (is_active, session_expired) = {
            let mut convos = self.conversations.lock().await;
            let active = convos.is_active(channel_id, user_id, now);
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

        self.handle_message(&ctx, &msg, bot_id, session_expired)
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
    ) {
        let mut text = msg.content.clone();
        for token in [format!("<@{bot_id}>"), format!("<@!{bot_id}>")] {
            text = text.replace(&token, "");
        }
        let text = text.trim().to_string();
        if text.is_empty() && msg.attachments.is_empty() {
            return;
        }

        if session_expired {
            self.agent
                .start_new_session(&msg.author.id.get().to_string())
                .await;
        }

        let images = extract_images(msg).await;

        // Post an immediate progress indicator.
        let progress = reply_no_ping(ctx, msg, "⚙️ **Generating...**").await.ok();
        let hooks = DiscordHooks::new(ctx.http.clone(), progress);

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
                &hooks,
            )
            .await;

        {
            let mut convos = self.conversations.lock().await;
            convos.mark_active(msg.channel_id.get(), msg.author.id.get(), Instant::now());
        }

        let safe = self.redactor.redact(&result.text);
        let (display, code_files) = extract_code_files(&safe);
        let progress = hooks.into_progress().await;
        send_final_message(ctx, msg, &display, progress).await;

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
}

fn split_command(content: &str) -> (String, String) {
    match content.split_once('\n') {
        Some((first, rest)) => (first.trim().to_string(), rest.trim().to_string()),
        None => (content.trim().to_string(), String::new()),
    }
}

async fn send_final_message(ctx: &Context, msg: &Message, text: &str, progress: Option<Message>) {
    let chunks = split_text(text, MAX_MESSAGE_LENGTH);
    if let Some(mut progress) = progress {
        if progress
            .edit(&ctx.http, EditMessage::new().content(&chunks[0]))
            .await
            .is_ok()
        {
            for chunk in &chunks[1..] {
                let _ = msg.channel_id.say(&ctx.http, chunk).await;
            }
            return;
        }
        let _ = progress.delete(&ctx.http).await;
    }
    for (i, chunk) in chunks.iter().enumerate() {
        if i == 0 {
            let _ = reply_no_ping(ctx, msg, chunk).await;
        } else {
            let _ = msg.channel_id.say(&ctx.http, chunk).await;
        }
    }
}

async fn extract_images(msg: &Message) -> Vec<ImageData> {
    let mut images = Vec::new();
    for att in &msg.attachments {
        let ext = att.filename.rsplit_once('.').map(|(_, e)| e.to_lowercase());
        let media_type = match ext.as_deref() {
            Some("png") => "image/png",
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => continue,
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

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// ── Discord progress hooks ───────────────────────────────────────────────────

struct DiscordHooks {
    http: Arc<serenity::http::Http>,
    progress: Mutex<Option<Message>>,
    last_edit: Mutex<Option<Instant>>,
    lines: Mutex<Vec<String>>,
    stream_started: AtomicBool,
}

impl DiscordHooks {
    fn new(http: Arc<serenity::http::Http>, progress: Option<Message>) -> Self {
        Self {
            http,
            progress: Mutex::new(progress),
            last_edit: Mutex::new(None),
            lines: Mutex::new(Vec::new()),
            stream_started: AtomicBool::new(false),
        }
    }

    async fn into_progress(self) -> Option<Message> {
        self.progress.into_inner()
    }

    async fn update(&self, content: String, force: bool) {
        {
            let last = self.last_edit.lock().await;
            if !force {
                if let Some(t) = *last {
                    if t.elapsed() < EDIT_INTERVAL {
                        return;
                    }
                }
            }
        }
        *self.last_edit.lock().await = Some(Instant::now());
        let mut guard = self.progress.lock().await;
        if let Some(m) = guard.as_mut() {
            let _ = m
                .edit(&self.http, EditMessage::new().content(content))
                .await;
        }
    }
}

#[async_trait::async_trait]
impl AgentHooks for DiscordHooks {
    async fn on_text_stream(&self, partial: &str) {
        let chunks = split_text(partial, MAX_MESSAGE_LENGTH);
        let mut content = chunks[0].clone();
        if chunks.len() > 1 {
            content.push('…');
        }
        let force = !self.stream_started.swap(true, Ordering::SeqCst);
        self.update(content, force).await;
    }

    async fn on_tool_called(&self, tool: &str, args: &Value) {
        let content = format!("⚙️ **`{tool}`**{}", tool_hint(tool, args));
        self.lines.lock().await.clear();
        self.update(content, true).await;
    }

    async fn on_progress(&self, line: &str) {
        let clean = line.trim();
        if clean.is_empty() {
            return;
        }
        let mut lines = self.lines.lock().await;
        lines.push(clean.to_string());
        let tail: String = lines
            .iter()
            .rev()
            .take(50)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        let tail: String = tail
            .chars()
            .rev()
            .take(1800)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let content = format!("⚙️ **Working...**\n```\n{tail}\n```");
        drop(lines);
        self.update(content, false).await;
    }
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
        t.mark_active(1, 2, now);
        assert!(t.is_active(1, 2, now + Duration::from_secs(100)));
    }

    #[test]
    fn tracker_pop_timed_out() {
        let mut t = ConversationTracker::new(Duration::from_secs(300));
        let now = Instant::now();
        t.mark_active(1, 2, now);
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
