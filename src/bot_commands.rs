//! Store-backed command handlers, independent of Discord transport.

use crate::history::History;
use crate::memory::Memory;
use crate::notes::Notes;
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
    format!("**Stats for {display_name}:**\n• Conversation history: {} messages ({turn_count} turns)\n• Memory size: {mem_kb:.1} KB\n• Saved notes: {}\n• Skills available: {}", hist.len(), user_notes.len(), all_skills.len())
}
