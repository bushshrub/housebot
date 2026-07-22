//! Store-backed command handlers, independent of Discord transport.

use housebot_bot_config::UserConfigStore;
use housebot_channel_log::ChannelLog;
use housebot_grocery::GroceryList;
use housebot_history::History;
use housebot_memory::Memory;
use housebot_message_log::MessageLog;
use housebot_notes::Notes;
use housebot_profile::ProfileStore;
use housebot_reminders::Reminders;
use housebot_skills::Skills;

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Parse a Discord user mention (`<@123>` or `<@!123>`) into a user ID string,
/// or return the raw string if it doesn't look like a mention.
fn parse_mention(raw: &str) -> &str {
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix("<@!").or_else(|| raw.strip_prefix("<@")) {
        if let Some(id) = inner.strip_suffix('>') {
            return id;
        }
    }
    raw
}

const SKILL_ADD_EDIT_REDIRECT: &str =
    "Skills are now created and edited by asking the bot directly in conversation \
     (it uses the create_skill / edit_skill tools) rather than through this command.";

/// `!skill list` / `/skill list`: every marketplace skill, marked with whether
/// `author_id` has it enabled.
pub async fn skill_list(skills: &Skills, user_config: &UserConfigStore, author_id: u64) -> String {
    let all = skills.load_all().await;
    if all.is_empty() {
        return "No skills in the marketplace yet. Ask the bot in conversation to create one."
            .into();
    }
    let enabled = user_config.load(author_id).await.enabled_skills;
    let mut lines = vec!["**Marketplace skills** (✓ = enabled for you):".to_string()];
    for skill in all.values() {
        let mark = if enabled.iter().any(|n| n == &skill.name) {
            "✓"
        } else {
            "•"
        };
        let author = skill
            .created_by
            .as_deref()
            .map(|id| format!(" <@{id}>"))
            .unwrap_or_default();
        lines.push(format!(
            "{} **{}** — {}{}",
            mark,
            skill.name,
            truncate_chars(skill.description_or_name(), 80),
            author,
        ));
    }
    lines.join("\n")
}

/// `!skill enable <name>`: opt `author_id` into a marketplace skill.
pub async fn skill_enable(
    skills: &Skills,
    user_config: &UserConfigStore,
    author_id: u64,
    name: &str,
) -> String {
    if skills.get(name).await.is_none() {
        return format!("Skill `{name}` not found in the marketplace.");
    }
    let mut cfg = user_config.load(author_id).await;
    if cfg.enabled_skills.iter().any(|n| n == name) {
        return format!("Skill **{name}** is already enabled.");
    }
    cfg.enabled_skills.push(name.to_string());
    if user_config.save(author_id, &cfg).await.is_err() {
        return "Error: failed to save your configuration.".into();
    }
    format!("✅ Skill **{name}** enabled.")
}

/// `!skill disable <name>`: opt `author_id` out of a previously enabled skill.
pub async fn skill_disable(user_config: &UserConfigStore, author_id: u64, name: &str) -> String {
    let mut cfg = user_config.load(author_id).await;
    let before = cfg.enabled_skills.len();
    cfg.enabled_skills.retain(|n| n != name);
    if cfg.enabled_skills.len() == before {
        return format!("Skill **{name}** was not enabled.");
    }
    if user_config.save(author_id, &cfg).await.is_err() {
        return "Error: failed to save your configuration.".into();
    }
    format!("✅ Skill **{name}** disabled.")
}

/// `!skill info <name>` / `/skill info <name>`: full details of one skill.
pub async fn skill_info(skills: &Skills, name: &str) -> String {
    match skills.get(name).await {
        None => format!("Skill `{name}` not found."),
        Some(skill) => {
            let instructions = skill.effective_instructions();
            let mut preview = truncate_chars(instructions, 500);
            if instructions.chars().count() > 500 {
                preview.push('…');
            }
            let author = skill
                .created_by
                .as_deref()
                .map(|id| format!("\n**Author:** <@{id}>"))
                .unwrap_or_default();
            let editors = if skill.editors.is_empty() {
                String::new()
            } else {
                let list: Vec<String> = skill.editors.iter().map(|id| format!("<@{id}>")).collect();
                format!("\n**Editors:** {}", list.join(", "))
            };
            let version = format!("\n**Version:** v{}", skill.version);
            let trigger_info = if skill.has_triggers() {
                let triggers: Vec<String> = skill
                    .triggers
                    .iter()
                    .map(|t| format!("{}: {}", t.trigger_type, t.value))
                    .collect();
                format!("\n**Triggers:** {}", triggers.join("; "))
            } else {
                String::new()
            };
            let tools_info = if skill.enabled_tools.is_empty() {
                String::new()
            } else {
                format!("\n**Tools:** {}", skill.enabled_tools.join(", "))
            };
            let example_count = skill.examples.len();
            let examples_info = if example_count > 0 {
                format!("\n**Examples:** {example_count}")
            } else {
                String::new()
            };
            format!(
                "**Skill: {}**\nDescription: {}{}{}{}{}{}{}\n```\n{}\n```",
                skill.name,
                skill.description.as_deref().unwrap_or("(none)"),
                author,
                editors,
                version,
                trigger_info,
                tools_info,
                examples_info,
                preview,
            )
        }
    }
}

/// `!skill delete <name>` / `/skill delete <name>`: author/editor-only delete.
pub async fn skill_delete(skills: &Skills, author_id: u64, name: &str) -> String {
    let author_str = author_id.to_string();
    match skills.get(name).await {
        None => format!("Skill `{name}` not found."),
        Some(skill) => {
            if !skill.can_edit(&author_str) {
                return format!(
                    "⛔ Only the author (<@{}>) or a delegated editor can delete **{name}**.",
                    skill.created_by.as_deref().unwrap_or("unknown")
                );
            }
            match skills.delete(name).await {
                Ok(true) => format!("✅ Skill **{name}** deleted."),
                _ => "Error: failed to delete skill.".into(),
            }
        }
    }
}

/// Author-only grant/revoke of delegated edit permission on a skill. `grant`
/// selects which of the two symmetric operations to perform.
async fn skill_delegate_editor(
    skills: &Skills,
    author_id: u64,
    name: &str,
    target_raw: &str,
    grant: bool,
) -> String {
    let author_str = author_id.to_string();
    let verb = if grant { "grant" } else { "revoke" };
    let target = parse_mention(target_raw);
    if target.parse::<u64>().is_err() {
        return "Please mention a valid user with @mention.".into();
    }
    match skills.get(name).await {
        None => format!("Skill `{name}` not found."),
        Some(mut skill) => {
            if !skill.is_author(&author_str) {
                return format!(
                    "⛔ Only the author (<@{}>) can {verb} edit permissions.",
                    skill.created_by.as_deref().unwrap_or("unknown")
                );
            }
            let changed = if grant {
                skill.add_editor(target)
            } else {
                skill.remove_editor(target)
            };
            if !changed {
                return if grant {
                    format!("<@{target}> can already edit **{name}**.")
                } else {
                    format!("<@{target}> does not have edit permission for **{name}**.")
                };
            }
            if skills.save(skill).await.is_err() {
                return "Error: failed to save skill.".into();
            }
            if grant {
                format!("✅ <@{target}> can now edit **{name}**.")
            } else {
                format!("✅ Removed <@{target}> from editors of **{name}**.")
            }
        }
    }
}

/// `!skill grant <name> <@user>`: let another user edit `name`.
pub async fn skill_grant(skills: &Skills, author_id: u64, name: &str, target_raw: &str) -> String {
    skill_delegate_editor(skills, author_id, name, target_raw, true).await
}

/// `!skill revoke <name> <@user>`: withdraw a previously granted edit right.
pub async fn skill_revoke(skills: &Skills, author_id: u64, name: &str, target_raw: &str) -> String {
    skill_delegate_editor(skills, author_id, name, target_raw, false).await
}

pub async fn skill_command(
    skills: &Skills,
    user_config: &UserConfigStore,
    first_line: &str,
    author_id: u64,
) -> String {
    let parts: Vec<&str> = first_line
        .splitn(4, char::is_whitespace)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() < 2 {
        return "Usage: `!skill list` | `!skill delete <name>` | `!skill info <name>` \
                | `!skill enable <name>` | `!skill disable <name>` \
                | `!skill grant <name> <@user>` | `!skill revoke <name> <@user>`\n\
                To create or edit a skill, just ask the bot in conversation."
            .into();
    }
    // Every subcommand below except `list` and `add`/`edit` takes a skill
    // name as its first argument; `require_name` extracts it or bails with
    // a usage string tailored to that subcommand.
    let require_name = |usage: &str| -> Result<String, String> {
        parts
            .get(2)
            .map(|s| s.to_lowercase())
            .ok_or_else(|| usage.to_string())
    };
    match parts[1].to_lowercase().as_str() {
        "list" => skill_list(skills, user_config, author_id).await,
        "enable" => match require_name("Usage: `!skill enable <name>`") {
            Ok(name) => skill_enable(skills, user_config, author_id, &name).await,
            Err(usage) => usage,
        },
        "disable" => match require_name("Usage: `!skill disable <name>`") {
            Ok(name) => skill_disable(user_config, author_id, &name).await,
            Err(usage) => usage,
        },
        "info" => match require_name("Usage: `!skill info <name>`") {
            Ok(name) => skill_info(skills, &name).await,
            Err(usage) => usage,
        },
        "add" | "edit" => SKILL_ADD_EDIT_REDIRECT.into(),
        "delete" => match require_name("Usage: `!skill delete <name>`") {
            Ok(name) => skill_delete(skills, author_id, &name).await,
            Err(usage) => usage,
        },
        "grant" => {
            let usage = "Usage: `!skill grant <name> <@user>`";
            match (require_name(usage), parts.get(3)) {
                (Ok(name), Some(target)) => skill_grant(skills, author_id, &name, target).await,
                _ => usage.into(),
            }
        }
        "revoke" => {
            let usage = "Usage: `!skill revoke <name> <@user>`";
            match (require_name(usage), parts.get(3)) {
                (Ok(name), Some(target)) => skill_revoke(skills, author_id, &name, target).await,
                _ => usage.into(),
            }
        }
        other => {
            format!(
                "Unknown subcommand `{other}`. Options: `list`, `add`, `edit`, `delete`, `info`, `enable`, `disable`, `grant`, `revoke`"
            )
        }
    }
}

pub async fn note_command(notes: &Notes, first_line: &str, rest: &str, author_id: u64) -> String {
    let parts: Vec<&str> = first_line
        .splitn(3, char::is_whitespace)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() < 2 {
        return "Usage: `/storage notes list` | `/storage notes save name:<name> content:<text>` | `/storage notes get name:<name>` | `/storage notes delete name:<name>`".into();
    }
    match parts[1].to_lowercase().as_str() {
        "list" => {
            let all = notes.load_all(author_id).await;
            if all.is_empty() {
                return "You have no saved notes. Use `/storage notes save name:<name> content:<text>` to create one.".into();
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
                return "Usage: `/storage notes get name:<name>`".into();
            };
            match notes.get(author_id, &name).await {
                None => format!("Note `{name}` not found."),
                Some(body) => format!("**{name}:**\n{body}"),
            }
        }
        "save" => {
            let Some(name) = parts.get(2).map(|s| s.trim().to_lowercase()) else {
                return "Usage: `/storage notes save name:<name> content:<text>`".into();
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
                return "Usage: `/storage notes delete name:<name>`".into();
            };
            match notes.delete(author_id, &name).await {
                Ok(true) => format!("✅ Note **{name}** deleted."),
                _ => format!("Note `{name}` not found."),
            }
        }
        other => {
            format!("Unknown subcommand `{other}`. Use `/storage notes list|save|get|delete`.")
        }
    }
}

pub async fn grocery_command(
    grocery: &GroceryList,
    first_line: &str,
    rest: &str,
    user_id: u64,
) -> String {
    let parts: Vec<&str> = first_line
        .splitn(3, char::is_whitespace)
        .filter(|s| !s.is_empty())
        .collect();
    match parts.get(1).copied() {
        Some("add") => {
            let item = if rest.is_empty() {
                parts.get(2).map(|s| s.trim()).unwrap_or("")
            } else {
                rest.trim()
            };
            if item.is_empty() {
                return "Usage: `!grocery add <item>`".into();
            }
            grocery
                .add(user_id, item)
                .await
                .unwrap_or_else(|e| format!("⚠️ Failed to add item: {e}"))
        }
        Some("remove") | Some("rm") => {
            let item = if rest.is_empty() {
                parts.get(2).map(|s| s.trim()).unwrap_or("")
            } else {
                rest.trim()
            };
            if item.is_empty() {
                return "Usage: `!grocery remove <item>`".into();
            }
            grocery
                .remove(user_id, item)
                .await
                .unwrap_or_else(|e| format!("⚠️ Failed to remove item: {e}"))
        }
        Some("flush") => grocery
            .flush(user_id)
            .await
            .unwrap_or_else(|e| format!("⚠️ Failed to flush list: {e}")),
        _ => grocery.display(user_id).await,
    }
}

/// Erase all stored data for the requesting user: message log, history, memory, notes, profile, reminders, and channel log entries.
#[allow(clippy::too_many_arguments)]
pub async fn erase_data_command(
    message_log: &MessageLog,
    history: &History,
    memory: &Memory,
    notes: &Notes,
    profile_store: &ProfileStore,
    user_config: &UserConfigStore,
    reminders: &Reminders,
    channel_log: &ChannelLog,
    grocery: &GroceryList,
    user_id: u64,
) -> String {
    let log_result = message_log.clear(user_id.to_string()).await;
    let history_result = history.clear(user_id.to_string()).await;
    let memory_result = memory.clear(user_id.to_string()).await;
    let notes_result = notes.clear(user_id.to_string()).await;
    let profile_result = profile_store.clear(user_id.to_string()).await;
    let config_result = user_config.clear(user_id).await;
    let _ = grocery.flush(user_id).await;

    // Remove user's reminders
    let mut all_reminders = reminders.load().await;
    let before = all_reminders.len();
    all_reminders.retain(|r| r.user_id != user_id.to_string());
    let removed_reminders = before.saturating_sub(all_reminders.len());
    let _ = reminders.store(&all_reminders).await;

    // Remove user's entries from channel logs (per-channel files)
    let channel_log_result = channel_log.remove_user_entries(user_id.to_string()).await;

    if log_result.is_err()
        || history_result.is_err()
        || memory_result.is_err()
        || notes_result.is_err()
        || profile_result.is_err()
        || config_result.is_err()
        || channel_log_result.is_err()
    {
        return "⚠️ Some data could not be erased. Please try again or contact an admin.".into();
    }

    let mut erased = vec![
        "message log",
        "conversation history",
        "memory",
        "notes",
        "profile",
        "configuration",
        "channel log entries",
    ];
    if removed_reminders > 0 {
        erased.push("reminders");
    }
    let erased_str = erased.join(", ");
    format!(
        "✅ All your stored data has been erased ({erased_str}). Your active session will also be cleared on next conversation start."
    )
}

pub async fn memory_command(memory: &Memory, first_line: &str, author_id: u64) -> String {
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 {
        return "Usage: `/storage memory show` | `/storage memory clear` | `/storage memory search query:<query>`".into();
    }
    match parts[1].to_lowercase().as_str() {
        "show" => {
            let content = memory.load(author_id.to_string()).await;
            if content.trim().is_empty() {
                "No memories stored yet. Enable deep memory with `/privacy deep_memory enabled:true`.".into()
            } else {
                truncate_discord("**What I remember about you:**\n", &content)
            }
        }
        "clear" => match memory.clear(author_id.to_string()).await {
            Ok(()) => "✅ Your memory has been cleared.".into(),
            Err(_) => "⚠️ Failed to clear memory. Please try again.".into(),
        },
        "search" => {
            let query = parts[2..].join(" ");
            if query.is_empty() {
                return "Usage: `/storage memory search query:<query>`".into();
            }
            let content = memory.load(author_id.to_string()).await;
            if content.trim().is_empty() {
                return "No memories stored yet.".into();
            }
            let query_lower = query.to_lowercase();
            let matching: Vec<&str> = content
                .lines()
                .filter(|line| line.to_lowercase().contains(&query_lower))
                .collect();
            if matching.is_empty() {
                truncate_discord("", &format!("No memories matching `{query}`."))
            } else {
                let header = format!("**Memories matching `{query}`:**\n");
                truncate_discord(&header, &matching.join("\n"))
            }
        }
        other => format!("Unknown subcommand `{other}`. Use `/storage memory show|clear|search`."),
    }
}

/// Prepend `header` to `body`, truncating the combined result to Discord's 2000-char limit.
fn truncate_discord(header: &str, body: &str) -> String {
    const LIMIT: usize = 2000;
    const ELLIPSIS: &str = "\n…(truncated)";
    let full = format!("{header}{body}");
    if full.chars().count() <= LIMIT {
        return full;
    }
    let keep = LIMIT.saturating_sub(ELLIPSIS.chars().count());
    let truncated: String = full.chars().take(keep).collect();
    format!("{truncated}{ELLIPSIS}")
}

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
        "**Stats for {display_name}:**\n• Conversation history: {} messages ({turn_count} turns)\n• Memory size: {mem_kb:.1} KB\n• Saved notes: {}\n• Skills available: {}",
        hist.len(),
        user_notes.len(),
        all_skills.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use housebot_bot_config::UserConfigStore;
    use housebot_profile::ProfileStore;
    use tempfile::TempDir;

    fn stores() -> (
        TempDir,
        MessageLog,
        History,
        Memory,
        Notes,
        ProfileStore,
        UserConfigStore,
        Reminders,
        ChannelLog,
        GroceryList,
    ) {
        let tmp = TempDir::new().unwrap();
        let msg_log = MessageLog::new(tmp.path().join("message_log"));
        let history = History::new(tmp.path().join("history"), 30);
        let memory = Memory::new(tmp.path().join("memories"));
        let notes = Notes::new(tmp.path().join("notes"));
        let profile = ProfileStore::new(tmp.path().join("profiles"));
        let user_config = UserConfigStore::new(tmp.path().join("user_config"));
        let reminders = Reminders::new(tmp.path().join("reminders.json"));
        let channel_log = ChannelLog::new(tmp.path().join("channel_log"));
        let grocery = GroceryList::new(tmp.path().join("grocery"));
        (
            tmp,
            msg_log,
            history,
            memory,
            notes,
            profile,
            user_config,
            reminders,
            channel_log,
            grocery,
        )
    }

    #[tokio::test]
    async fn erase_data_clears_all_stores() {
        let (
            _tmp,
            msg_log,
            history,
            memory,
            notes,
            profile,
            user_config,
            reminders,
            channel_log,
            grocery,
        ) = stores();
        let user_id = 123u64;

        // Populate all stores
        msg_log.append(user_id.to_string(), "test").await;
        history
            .save(
                user_id.to_string(),
                &[serde_json::json!({"role":"user","content":"hi"})],
            )
            .await
            .unwrap();
        memory
            .save(user_id.to_string(), "some memory")
            .await
            .unwrap();
        notes.save(user_id, "test", "content").await.unwrap();
        profile
            .save(
                user_id.to_string(),
                &housebot_profile::UserProfile {
                    username: "alice".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        user_config
            .save(
                user_id,
                &housebot_bot_config::UserConfig {
                    deep_memory_enabled: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        reminders
            .add(
                &user_id.to_string(),
                "reminder",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs_f64()
                    + 60.0,
            )
            .await
            .unwrap();
        channel_log
            .append(1, user_id, "Alice", None, "channel msg")
            .await;
        grocery.add(user_id, "milk").await.unwrap();

        let reply = erase_data_command(
            &msg_log,
            &history,
            &memory,
            &notes,
            &profile,
            &user_config,
            &reminders,
            &channel_log,
            &grocery,
            user_id,
        )
        .await;

        assert!(reply.contains("erased"));
        assert!(reply.contains("message log"));
        assert!(reply.contains("conversation history"));
        assert!(reply.contains("memory"));
        assert!(reply.contains("notes"));
        assert!(reply.contains("profile"));
        assert!(reply.contains("reminders"));

        // Verify stores are cleared
        assert!(history.load(user_id.to_string()).await.is_empty());
        assert_eq!(memory.load(user_id.to_string()).await, "");
        assert!(notes.load_all(user_id).await.is_empty());
        assert_eq!(profile.load(user_id.to_string()).await.username, "");
        assert!(user_config.load(user_id).await.deep_memory_enabled);
        assert!(reminders.load().await.is_empty());
        assert!(grocery.load(user_id).await.is_empty());
    }

    #[tokio::test]
    async fn erase_data_preserves_other_users() {
        let (
            _tmp,
            msg_log,
            history,
            memory,
            notes,
            profile,
            user_config,
            reminders,
            channel_log,
            grocery,
        ) = stores();
        let user_a = 100u64;
        let user_b = 200u64;

        // Populate stores with both users
        msg_log.append(user_a.to_string(), "a").await;
        msg_log.append(user_b.to_string(), "b").await;
        history
            .save(
                user_a.to_string(),
                &[serde_json::json!({"role":"user","content":"a"})],
            )
            .await
            .unwrap();
        history
            .save(
                user_b.to_string(),
                &[serde_json::json!({"role":"user","content":"b"})],
            )
            .await
            .unwrap();
        reminders
            .add(
                &user_a.to_string(),
                "reminder_a",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs_f64()
                    + 60.0,
            )
            .await
            .unwrap();
        reminders
            .add(
                &user_b.to_string(),
                "reminder_b",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs_f64()
                    + 60.0,
            )
            .await
            .unwrap();
        channel_log.append(1, user_a, "Alice", None, "msg a").await;
        channel_log.append(1, user_b, "Bob", None, "msg b").await;

        // Erase user A
        erase_data_command(
            &msg_log,
            &history,
            &memory,
            &notes,
            &profile,
            &user_config,
            &reminders,
            &channel_log,
            &grocery,
            user_a,
        )
        .await;

        // Verify user B is preserved
        assert_eq!(history.load(user_b.to_string()).await.len(), 1);
        let remaining_reminders = reminders.load().await;
        assert_eq!(remaining_reminders.len(), 1);
        assert_eq!(remaining_reminders[0].user_id, user_b.to_string());
    }
}
