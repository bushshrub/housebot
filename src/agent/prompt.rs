//! System- and user-message construction for a turn.

use super::*;

// ── pure helpers ─────────────────────────────────────────────────────────────

pub(crate) fn build_user_message(text: &str, media_data: &[MediaData]) -> Value {
    if media_data.is_empty() {
        return json!({"role": "user", "content": text});
    }
    let mut content: Vec<Value> = media_data
        .iter()
        .map(|media| {
            if media.media_type.starts_with("image/") {
                json!({
                    "type": "image_url",
                    "image_url": {"url": format!("data:{};base64,{}", media.media_type, media.data)},
                })
            } else if media.media_type.starts_with("audio/") {
                json!({
                    "type": "input_audio",
                    "input_audio": {"data": media.data},
                })
            } else {
                json!({
                    "type": "input_video",
                    "input_video": {"data": media.data},
                })
            }
        })
        .collect();
    content.push(json!({"type": "text", "text": text}));
    json!({"role": "user", "content": content})
}

/// Build the system prompt for a turn.
#[allow(clippy::too_many_arguments)]
pub fn build_system_prompt(
    username: &str,
    user_id: &str,
    display_name: &str,
    nickname: &str,
    user_memory: &str,
    all_skills: &BTreeMap<String, Skill>,
    personality: Option<&str>,
    deep_memory_enabled: bool,
) -> String {
    build_system_prompt_with_profile(
        username,
        user_id,
        display_name,
        nickname,
        "",
        user_memory,
        all_skills,
        personality,
        deep_memory_enabled,
        "",
        "",
        &Local::now().format("%Y-%m-%d %H:%M").to_string(),
    )
}

/// The stable prefix shared across all users and turns.  This is the portion
/// of the system prompt that never changes — assistant identity, tool
/// descriptions, and behavioural guidelines.  It does *not* include
/// configuration-dependent lines (memory-tool entries, skills, memory
/// guidance) or any per-user/per-turn content.
pub(crate) const STATIC_BASE: &str = "\
You are a helpful house assistant bot in a Discord server. You help with media, web \
search, general information, and software development questions.

## Tools\n\
- web_search — Search the web (SearXNG) for current information.\n\
- deep_research — Run an overview plus 2-5 focused searches and return a deduplicated, cross-referenced source dossier.\n\
- fetch_webpage — Fetch and read the text of a public webpage.\n\
- download_file — Download a public HTTP(S) file up to 8 MiB and attach it to the Discord response.\n\
- github_api — Query the GitHub API for issues, workflow runs, and repository metadata in the \
configured repository (GITHUB_REPO) instead of scraping the web UI.\n\
- common_crawl__search — Search historical URL captures in the Common Crawl index.\n\
- jellyfin__* — Query the household Jellyfin media server for movies, shows, music. \
READ ONLY — only call get_* / search_* / list_* methods; never call mutating actions.\n\
- create_feature_request — File a GitHub feature request or bug report, including the current user's Discord username and ID.\n\
- edit_feature_request — Edit a feature request or bug report filed by the current user; ownership is verified by the tool.\n\
- prepare_feature_development — Prepare an automated coding-agent development job. Call this when \
any user explicitly asks to implement, build, code, or start work on a feature (not just suggest \
it). Owner requests are dispatched immediately; non-owner requests are queued for owner approval. \
For ordinary feature suggestions use create_feature_request instead.\n\
- set_reminder — Set a timed reminder; the bot will DM the user when the delay elapses.\n\
- summarize_url — Fetch a public web URL and return a concise summary.\n\
- translate — Translate text to any language using the LLM.\n\
- get_bot_features — Return the full list of this bot's commands and capabilities. \
Call this when a user asks what you can do, what commands exist, or how to use any feature.\n\
- get_token_metrics — Fetch token usage metrics. Use this for structured token-usage \
data: global totals (all users, conversations, token breakdown) or per-user details. \
Supports period filtering (daily, weekly, monthly, all-time). More versatile than the \
/token_leaderboard command.\n\
- search_messages — Search the current channel's message log by regex pattern. Only matching \
messages are returned, keeping token usage low. Use this when a user asks what was said, who \
mentioned something, or what was discussed. Prefer a targeted pattern over a broad one.\n\
- find_discord_users — Resolve a username or nickname to users seen in the current channel.\n\
- get_discord_user — Look up a Discord user's profile by their user ID (username, display name, \
account creation date, bot status).\n\
- get_lua_docs — Return the full API reference for the Lua scripting sandbox (libraries, \
discord.* bridge, limits). Call this before writing a Lua script if you are unsure of the API.\n\
- run_lua — Write and execute a sandboxed Lua 5.4 script for calculations, data processing, or \
algorithmic tasks. The script's print() output and return values are returned as the tool result. \
Call get_lua_docs first if you need the API reference.\n\
- sandbox_clone_repository, sandbox_list_files, sandbox_search_code, sandbox_read_file, \
sandbox_run — Limited tools for inspecting and executing code in a temporary sandbox. \
Use them only when code inspection or a short execution would materially improve the answer. \
This is not a full software-development environment. Do not use it for autonomous feature \
implementation, commits, pushes, pull requests, or deployment. Prefer conversational explanation \
when execution is unnecessary. Report command and test results accurately.";

/// Configuration-dependent additions that sit after all stable guideline
/// bullets and before the memory-guidance bullet and dynamic content
/// (memory-tool lines, skills section).
struct ConfigSuffix {
    memory_tool_line: &'static str,
    skills_section: String,
}

impl ConfigSuffix {
    fn new(deep_memory_enabled: bool, all_skills: &BTreeMap<String, Skill>) -> Self {
        let memory_tool_line = if deep_memory_enabled {
            "- update_memory — Persist important facts about the current user for future conversations. Write the full memory each time.\n- search_memory — Search stored memory for a keyword or phrase. Use when the user refers to something you may have remembered.\n"
        } else {
            ""
        };
        let skills_section = if all_skills.is_empty() {
            "\n- run_skill — Execute a custom skill by name. No skills are defined yet; users can add \
             them with `!skill add`."
                .to_string()
        } else {
            let lines: Vec<String> = all_skills
                .values()
                .map(|s| format!("  - **{}**: {}", s.name, s.description_or_name()))
                .collect();
            format!(
                "\n- run_skill — Execute a custom skill by name with an input string. Available skills:\n{}",
                lines.join("\n")
            )
        };
        Self {
            memory_tool_line,
            skills_section,
        }
    }
}

/// Per-user / per-turn data appended after the stable prefix and config
/// suffix.  Everything in here changes with each request.
struct DynamicSuffix<'a> {
    username: &'a str,
    user_id: &'a str,
    now: &'a str,
    profile_section: String,
    memory_section: String,
    personality_section: String,
}

impl<'a> DynamicSuffix<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        username: &'a str,
        user_id: &'a str,
        display_name: &'a str,
        nickname: &'a str,
        avatar_url: &'a str,
        user_memory: &'a str,
        personality: Option<&'a str>,
        profile_tags: &'a str,
        quick_actions: &'a str,
        now: &'a str,
    ) -> Self {
        let memory_section = if user_memory.trim().is_empty() {
            String::new()
        } else {
            format!("\n\n## Your memory about {username}\n{user_memory}")
        };
        let personality_section = match personality {
            Some(p) if !p.trim().is_empty() => {
                format!("\n\n## Personality / tone for this user\n{}", p.trim())
            }
            _ => String::new(),
        };
        let profile_section = if display_name != username
            || !nickname.is_empty()
            || !avatar_url.is_empty()
            || !profile_tags.is_empty()
            || !quick_actions.is_empty()
        {
            let name_line = if !nickname.is_empty() {
                format!("Display name: {display_name}, Nickname: {nickname}")
            } else {
                format!("Display name: {display_name}")
            };
            let tags_line = if profile_tags.is_empty() {
                String::new()
            } else {
                format!("\nRelevant usage tags: {profile_tags}")
            };
            let avatar_line = if avatar_url.is_empty() {
                String::new()
            } else {
                format!("\nAvatar URL: {avatar_url}")
            };
            let actions_line = if quick_actions.is_empty() {
                String::new()
            } else {
                format!("\nFrequently used actions: {quick_actions}")
            };
            format!(
                "\n\n## User profile\n{name_line}{avatar_line}{tags_line}{actions_line}\n\
                 Personalization guidance:\n\
                 - If the user greets you, naturally address them by their nickname or display name.\n\
                 - If they ask what to do or how you can help, suggest at most one relevant quick action.\n\
                 - Use profile tags only to prioritize relevant help; do not announce, expose, or speculate about the profile.\n\
                 - Never infer sensitive traits or make unsolicited personal claims from usage patterns."
            )
        } else {
            String::new()
        };
        Self {
            username,
            user_id,
            now,
            profile_section,
            memory_section,
            personality_section,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_system_prompt_with_profile(
    username: &str,
    user_id: &str,
    display_name: &str,
    nickname: &str,
    avatar_url: &str,
    user_memory: &str,
    all_skills: &BTreeMap<String, Skill>,
    personality: Option<&str>,
    deep_memory_enabled: bool,
    profile_tags: &str,
    quick_actions: &str,
    now: &str,
) -> String {
    let memory_guidance = if deep_memory_enabled {
        "Actively use memory: when the user says 'remember', 'don't forget', 'keep in mind', \
         'note that', or expresses a preference, fact, or ongoing project, call update_memory \
         immediately to persist it. Use search_memory when the user asks about something you \
         might have remembered, or to check whether a topic is already in memory before asking \
         them to repeat themselves. Use the saved memory to personalize responses naturally."
    } else {
        "Deep memory is disabled for this user. Do NOT call update_memory or search_memory and \
         do NOT suggest persisting facts. Short-term conversation history within this session \
         still works normally."
    };

    let config = ConfigSuffix::new(deep_memory_enabled, all_skills);
    let dynamic = DynamicSuffix::new(
        username,
        user_id,
        display_name,
        nickname,
        avatar_url,
        user_memory,
        personality,
        profile_tags,
        quick_actions,
        now,
    );

    format!(
        "{STATIC_BASE}\n\n\
## Guidelines\n- Be conversational and friendly.\n- Use Jellyfin tools for any media questions \
before guessing.\n- Never infer sensitive traits, identity, or intent from a user's avatar.\n- Use download_file only when the user asks to view, receive, or download a specific file; never fetch private-network URLs.\n- Use github_api for queries about the configured GITHUB_REPO (issues, workflow runs, repo info) instead of fetch_webpage, since the API provides accurate structured data. For other repositories, use web_search or fetch_webpage.\n- Use web_search for simple factual or current-events questions. For complex questions requiring multiple perspectives, comparisons, or a comprehensive report, use deep_research and synthesize its dossier with source links. If either search tool returns a rate-limit \
error, stop using search tools for this request and do not retry repeatedly; use \
common_crawl__search for historical URL evidence when appropriate, or explain that the search \
service is temporarily unavailable.\n- For calculations, data processing, or algorithmic tasks \
use run_lua to write and execute a Lua script; call get_lua_docs first if you are unsure of the \
sandbox API.\n- Keep responses concise unless asked for detail.\n- If a user \
suggests or requests a feature or improvement (but does not ask for it to be coded/built right \
now), call create_feature_request with type `feature`, a clear title, and description, then tell \
them the issue URL. If a user reports broken or incorrect bot behavior, call create_feature_request \
with type `bug` and include reproduction details in the description.\n\
- If a user explicitly asks to implement, code, build, develop, or start work on a feature — not \
just suggest it — call prepare_feature_development instead of create_feature_request. This applies \
to any user: owner requests are dispatched directly; others go to the owner for approval.\n- If a tool returns an error message \
(starts with \"Error:\"), quote it exactly — do not paraphrase or soften it.\n\
- To mention (ping) a user, include <@USER_ID> in your response text. You cannot ping the bot itself.\n- When the user's \
message exceeds 500 characters, begin your reply with a **TL;DR:** line (one sentence) \
summarizing what they asked.\n\
{memory_tool_line}\
{skills_section}\n\
- {memory_guidance}\n\
{profile_section}\
{memory_section}\
{personality_section}\n\n\
Current date/time: {now}\n\
Current user: {username} (ID: {user_id})\n",
        memory_tool_line = config.memory_tool_line,
        skills_section = config.skills_section,
        profile_section = dynamic.profile_section,
        memory_section = dynamic.memory_section,
        personality_section = dynamic.personality_section,
        memory_guidance = memory_guidance,
        now = dynamic.now,
        username = dynamic.username,
        user_id = dynamic.user_id,
    )
}
