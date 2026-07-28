//! User data.

use crate::*;
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
    let grocery_result = grocery.flush(user_id).await;

    // Remove user's reminders
    let mut all_reminders = reminders.load().await;
    let before = all_reminders.len();
    all_reminders.retain(|r| r.user_id != user_id.to_string());
    let removed_reminders = before.saturating_sub(all_reminders.len());
    let reminders_result = reminders.store(&all_reminders).await;

    // Remove user's entries from channel logs (per-channel files)
    let channel_log_result = channel_log.remove_user_entries(user_id.to_string()).await;

    if log_result.is_err()
        || history_result.is_err()
        || memory_result.is_err()
        || notes_result.is_err()
        || profile_result.is_err()
        || config_result.is_err()
        || channel_log_result.is_err()
        || grocery_result.is_err()
        || reminders_result.is_err()
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
