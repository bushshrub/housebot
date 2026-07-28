//! Interactions data.

//! Slash-command interaction handlers (effort, tool bans, status, data, privacy, skill, stats).

use super::*;

/// Handle a `/status` interaction: show the user's current settings at a glance.
pub(crate) async fn handle_status_interaction(
    user_cfg: &UserConfigStore,
    author_id: u64,
) -> String {
    let cfg = user_cfg.load(author_id).await;
    let effort = format!(
        "**{}** — {}",
        cfg.thinking_mode,
        cfg.thinking_mode.budget_label()
    );
    let followup = if cfg.followup_enabled {
        format!("enabled (timeout: {}s)", cfg.followup_timeout_secs)
    } else {
        "disabled".to_string()
    };
    let personality = match &cfg.personality {
        Some(p) if !p.trim().is_empty() => format!("> {}", p.trim().replace('\n', "\n> ")),
        _ => "default".to_string(),
    };
    let progress = if cfg.progress_updates_enabled {
        "enabled"
    } else {
        "disabled (final responses only)"
    };
    format!(
        "**Your current settings:**\n• Effort level: {effort}\n• Progress updates: {progress}\n• Follow-up replies: {followup}\n• Personality: {personality}\n\nUse `/effort` to change the thinking effort level."
    )
}

/// Handle `/data history`: show or clear history.
pub(crate) async fn handle_history_interaction(
    history: &History,
    profile_store: &ProfileStore,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
    _guild_id: Option<u64>,
) -> String {
    let profile = profile_store.load(author_id).await;
    let name = profile.best_name();
    let subcommand = options.first().map(|o| o.name.as_str());
    match subcommand {
        Some("clear") => {
            let _ = history.clear(author_id.to_string()).await;
            format!("✅ Conversation history cleared for {name}.")
        }
        _ => {
            let hist = history.load(author_id.to_string()).await;
            render_history(&profile, &hist)
        }
    }
}

pub(crate) fn render_history(
    profile: &crate::profile::UserProfile,
    hist: &[serde_json::Value],
) -> String {
    let name = profile.best_name();
    let mut lines = vec![
        format!("**History for {name}**"),
        "Scope: all servers and channels where you used housebot".to_string(),
    ];

    let profile_bits: Vec<String> = profile
        .tags
        .iter()
        .map(|tag| tag.as_str().to_string())
        .collect();
    if !profile_bits.is_empty() {
        lines.push(format!("Profile interests: {}", profile_bits.join(", ")));
    }

    if hist.is_empty() {
        lines.push("No conversation history yet.".to_string());
        return lines.join("\n");
    }

    let turn_count = hist
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        .count();
    let mut recent: Vec<&serde_json::Value> = hist
        .iter()
        .rev()
        .filter(|m| m.get("content").and_then(|c| c.as_str()).is_some())
        .take(10)
        .collect();
    recent.reverse();

    lines.push(format!(
        "Total messages: {} ({} turns)",
        hist.len(),
        turn_count
    ));
    lines.push("Recent interactions:".to_string());
    for msg in recent {
        let role = msg["role"].as_str().unwrap_or("?");
        let content = msg["content"].as_str().unwrap_or("");
        let preview: String = content.chars().take(80).collect();
        let location = msg
            .get("discord_context")
            .and_then(|ctx| ctx.get("channel_id"))
            .and_then(|id| id.as_u64())
            .map(|id| format!(" in <#{id}>"))
            .unwrap_or_default();
        let timestamp = msg
            .get("discord_context")
            .and_then(|ctx| ctx.get("timestamp"))
            .and_then(|value| value.as_str())
            .and_then(|value| value.get(..10))
            .map(|date| format!(" on {date}"))
            .unwrap_or_default();
        lines.push(format!("[{role}{location}{timestamp}] {preview}"));
    }
    if hist.len() > 10 {
        lines.push(format!("... and {} more messages", hist.len() - 10));
    }
    lines.join("\n")
}

/// Handle a `/privacy` interaction: view or change privacy settings.
pub(crate) async fn handle_privacy_interaction(
    user_cfg: &UserConfigStore,
    memory: &Memory,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
) -> String {
    let subcommand = options.first().map(|o| o.name.as_str());
    match subcommand {
        None | Some("status") => {
            let cfg = user_cfg.load(author_id).await;
            let mem_content = memory.load(author_id.to_string()).await;
            let deep_memory = if cfg.deep_memory_enabled {
                if mem_content.trim().is_empty() {
                    "enabled (no memories stored yet)".to_string()
                } else {
                    format!(
                        "enabled ({} bytes stored — use `/storage memory show` to view)",
                        mem_content.len()
                    )
                }
            } else {
                "disabled".to_string()
            };
            format!(
                "**Privacy settings:**\n• Deep memory: {deep_memory} (persistent facts across sessions)\n\nUse `/privacy deep_memory enabled:true` to change. Proactive assistance moved to `/personalize proactive`."
            )
        }
        Some("deep_memory") => {
            let sub_opts = match &options[0].value {
                serenity::all::CommandDataOptionValue::SubCommand(opts) => opts,
                _ => return "Unexpected option structure.".into(),
            };
            let enabled =
                sub_opts
                    .iter()
                    .find(|o| o.name == "enabled")
                    .and_then(|o| match &o.value {
                        serenity::all::CommandDataOptionValue::Boolean(b) => Some(*b),
                        _ => None,
                    });
            let Some(enabled) = enabled else {
                return "Please specify `enabled`.".into();
            };
            let mut cfg = user_cfg.load(author_id).await;
            cfg.deep_memory_enabled = enabled;
            if user_cfg.save(author_id, &cfg).await.is_err() {
                return "Error: failed to save config.".into();
            }
            if enabled {
                "✅ Deep memory enabled. I will now remember important facts about you across conversations. Use `/storage memory show` to see what I currently remember.".into()
            } else {
                "✅ Deep memory disabled. I will no longer save facts between sessions (your current memories are kept but won't be updated).".into()
            }
        }
        Some("proactive") => {
            "Proactive assistance moved to `/personalize proactive enabled:<true|false>`.".into()
        }
        other => {
            format!("Unknown privacy option `{other:?}`. Use `/privacy` to see available options.")
        }
    }
}

pub(crate) async fn handle_skill_interaction(
    skills: &Skills,
    user_cfg: &UserConfigStore,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
) -> String {
    let Some(command) = options.first() else {
        return "Usage: `/skill list` | `/skill info <name>` | `/skill delete <name>`. To create or edit a skill, ask the bot in conversation.".into();
    };
    let sub_opts = match &command.value {
        CommandDataOptionValue::SubCommand(opts) => opts,
        _ => return "Unexpected option structure.".into(),
    };
    let name_option = |opts: &[serenity::all::CommandDataOption]| {
        opts.iter()
            .find(|o| o.name == "name")
            .and_then(|o| match &o.value {
                CommandDataOptionValue::String(s) => Some(s.to_lowercase()),
                _ => None,
            })
            .unwrap_or_default()
    };
    match command.name.as_str() {
        "list" => skill_list(skills, user_cfg, author_id).await,
        "info" => skill_info(skills, &name_option(sub_opts)).await,
        "delete" => skill_delete(skills, author_id, &name_option(sub_opts)).await,
        other => format!("Unknown subcommand `{other}`. Options: list, info, delete"),
    }
}

pub(crate) async fn handle_stats_interaction(
    history: &History,
    memory: &Memory,
    notes: &Notes,
    skills: &Skills,
    author_id: u64,
    display_name: &str,
) -> String {
    stats_command(history, memory, notes, skills, author_id, display_name).await
}
