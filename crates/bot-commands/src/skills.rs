//! Skills.

use crate::*;
use housebot_bot_config::UserConfigStore;
use housebot_skills::Skills;

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
    let parts: Vec<&str> = first_line.split_whitespace().collect();
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
