//! Store-backed command handlers, independent of Discord transport.

use crate::bot_config::UserConfigStore;
use crate::channel_log::ChannelLog;
use crate::history::History;
use crate::memory::Memory;
use crate::message_log::MessageLog;
use crate::notes::Notes;
use crate::profile::ProfileStore;
use crate::reminders::Reminders;
use crate::skills::{Skill, Skills};

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

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
            for skill in all.values() {
                lines.push(format!(
                    "• **{}** — {}",
                    skill.name,
                    truncate_chars(skill.description_or_name(), 80)
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
                Some(skill) => {
                    let mut preview = truncate_chars(&skill.prompt, 500);
                    if skill.prompt.chars().count() > 500 {
                        preview.push('…');
                    }
                    format!(
                        "**Skill: {}**\nDescription: {}\n```\n{}\n```",
                        skill.name,
                        skill.description.as_deref().unwrap_or("(none)"),
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
    user_id: u64,
) -> String {
    let log_result = message_log.clear(user_id.to_string()).await;
    let history_result = history.clear(user_id.to_string()).await;
    let memory_result = memory.clear(user_id.to_string()).await;
    let notes_result = notes.clear(user_id.to_string()).await;
    let profile_result = profile_store.clear(user_id.to_string()).await;
    let config_result = user_config.clear(user_id).await;

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
    use crate::bot_config::UserConfigStore;
    use crate::profile::ProfileStore;
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
        )
    }

    #[tokio::test]
    async fn erase_data_clears_all_stores() {
        let (_tmp, msg_log, history, memory, notes, profile, user_config, reminders, channel_log) =
            stores();
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
                &crate::profile::UserProfile {
                    username: "alice".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        user_config
            .save(
                user_id,
                &crate::bot_config::UserConfig {
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

        let reply = erase_data_command(
            &msg_log,
            &history,
            &memory,
            &notes,
            &profile,
            &user_config,
            &reminders,
            &channel_log,
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
        assert!(!user_config.load(user_id).await.deep_memory_enabled);
        assert!(reminders.load().await.is_empty());
    }

    #[tokio::test]
    async fn erase_data_preserves_other_users() {
        let (_tmp, msg_log, history, memory, notes, profile, user_config, reminders, channel_log) =
            stores();
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
