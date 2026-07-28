//! System- and user-message assembly.

//! System- and user-message construction for a turn.

use super::*;

// ── pure helpers ─────────────────────────────────────────────────────────────
pub(crate) use super::prompt_base::*;
use super::prompt_suffix::*;

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
        "",
    )
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
    current_message: &str,
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

    let config = ConfigSuffix::new(deep_memory_enabled, all_skills, current_message);
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
## Guidelines\n- Be direct and straightforward. Do not pander, flatter, apologize unnecessarily, or \
validate the user's emotional state — respond to what they say, not how they say it.\n\
- Use Jellyfin tools for any media questions before guessing.\n- Never infer sensitive traits, identity, or intent from a user's avatar.\n- Use download_file only when the user asks to view, receive, or download a specific file; never fetch private-network URLs.\n- Use github_api for queries about the configured GITHUB_REPO (issues, workflow runs, repo info) instead of fetch_webpage, since the API provides accurate structured data. For other repositories, use web_search or fetch_webpage.\n- Use web_search for simple factual or current-events questions. For complex questions requiring multiple perspectives, comparisons, or a comprehensive report, use deep_research and synthesize its dossier with source links. If either search tool returns a rate-limit \
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
- When a user asks what was discussed, what happened, or to recap — or says something vague \
like 'what were we talking about' — call get_messages (mode=recent) to fetch recent channel \
history before answering. Use mode=search only when they ask about a specific keyword, topic, or \
person. When a user replies to a message and asks about the surrounding conversation, use \
mode=before/after/around with that message's ID.\n\n\
## Session information\n\
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

/// Render a skill's loaded content for injection into the main agent's
/// context: its instructions, recommended tools, and few-shot examples.
pub(crate) fn build_loaded_skill_content(skill: &Skill, instructions: &str) -> String {
    let mut parts = vec![format!(
        "# Skill: {}\nYou have loaded the **{}** skill. Follow these instructions using your \
         normal tools.\n\n{instructions}",
        skill.name, skill.name
    )];

    if !skill.enabled_tools.is_empty() {
        parts.push(format!(
            "## Recommended tools\nThis skill is intended to use: {}.",
            skill.enabled_tools.join(", ")
        ));
    }

    if !skill.examples.is_empty() {
        let examples: Vec<String> = skill
            .examples
            .iter()
            .map(|ex| {
                format!(
                    "User: {}\nAssistant: {}",
                    ex.input.replace('\n', "\n  "),
                    ex.output.replace('\n', "\n  ")
                )
            })
            .collect();
        parts.push(format!("## Examples\n{}", examples.join("\n\n")));
    }

    parts.join("\n\n")
}
