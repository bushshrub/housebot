//! Per-request suffixes appended to the static system prompt.

//! System- and user-message construction for a turn.

use super::*;

// ── pure helpers ─────────────────────────────────────────────────────────────

/// Configuration-dependent additions that sit after all stable guideline
/// bullets and before the memory-guidance bullet and dynamic content
/// (memory-tool lines, skills section).
pub(crate) struct ConfigSuffix {
    pub(crate) memory_tool_line: &'static str,
    pub(crate) skills_section: String,
}

impl ConfigSuffix {
    pub(crate) fn new(
        deep_memory_enabled: bool,
        all_skills: &BTreeMap<String, Skill>,
        current_message: &str,
    ) -> Self {
        let memory_tool_line = if deep_memory_enabled {
            "- update_memory — Persist important facts about the current user for future conversations. Write the full memory each time.\n- search_memory — Search stored memory for a keyword or phrase. Use when the user refers to something you may have remembered.\n"
        } else {
            ""
        };
        let skills_section = if all_skills.is_empty() {
            "\n- use_skill — Load a custom skill's instructions into your context by name. You have \
              no skills enabled yet; browse the marketplace with list_skills and enable one with \
              enable_skill (or `!skill enable <name>`), or create one through conversation."
                .to_string()
        } else {
            // Only skill names appear here — user-authored descriptions must not
            // receive system-message authority. Use list_skills / skill_info to
            // inspect a skill's details as tool output instead.
            let lines: Vec<String> = all_skills
                .values()
                .map(|s| format!("  - **{}**", s.name))
                .collect();
            let mut section = format!(
                "\n- use_skill — Load a custom skill's full instructions into your context by name, \
                 then follow them yourself using your normal tools. Use list_skills or skill_info \
                 to see what a skill does. Available skills:\n{}",
                lines.join("\n")
            );
            let matched: Vec<String> = all_skills
                .values()
                .filter(|s| s.matches_message(current_message))
                .map(|s| format!("**{}**", s.name))
                .collect();
            if !matched.is_empty() {
                section.push_str(&format!(
                    "\n\nThe current message matches the triggers of these skills — strongly \
                     consider loading them with use_skill: {}.",
                    matched.join(", ")
                ));
            }
            section
        };
        Self {
            memory_tool_line,
            skills_section,
        }
    }
}

/// Per-user / per-turn data appended after the stable prefix and config
/// suffix.  Everything in here changes with each request.
pub(crate) struct DynamicSuffix<'a> {
    pub(crate) username: &'a str,
    pub(crate) user_id: &'a str,
    pub(crate) now: &'a str,
    pub(crate) profile_section: String,
    pub(crate) memory_section: String,
    pub(crate) personality_section: String,
}

impl<'a> DynamicSuffix<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
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
