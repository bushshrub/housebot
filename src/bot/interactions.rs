//! Slash-command interaction handlers (effort, tool bans, status, data, privacy, skill, stats).

use super::*;

pub(crate) async fn handle_effort_interaction(
    user_cfg: &UserConfigStore,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
) -> String {
    let level = options
        .iter()
        .find(|o| o.name == "level")
        .and_then(|o| match &o.value {
            CommandDataOptionValue::String(s) => Some(s.clone()),
            _ => None,
        });
    let mut cfg = user_cfg.load(author_id).await;
    let Some(level) = level else {
        let lines: Vec<String> = ThinkingMode::ALL
            .into_iter()
            .map(|mode| {
                let marker = if mode == cfg.thinking_mode {
                    " ←"
                } else {
                    ""
                };
                format!("• **{mode}** — {}{marker}", mode.budget_label())
            })
            .collect();
        return format!(
            "**Thinking effort:** currently **{}** ({}).\n{}\nUse `/effort level:<mode>` to change it.",
            cfg.thinking_mode,
            cfg.thinking_mode.budget_label(),
            lines.join("\n")
        );
    };
    let Ok(mode) = level.parse::<ThinkingMode>() else {
        return format!("Unknown effort level `{level}`. Options: low, medium, high, xhigh, max.");
    };
    cfg.thinking_mode = mode;
    if let Err(error) = user_cfg.save(author_id, &cfg).await {
        tracing::error!(target: "housebot::commands", user_id = author_id, %error, "Failed to save effort setting");
        return "Error: failed to save config.".into();
    }
    tracing::info!(target: "housebot::commands", user_id = author_id, mode = %mode, "Thinking effort updated");
    format!(
        "✅ Thinking effort set to **{mode}** ({}).",
        mode.budget_label()
    )
}

/// Handle guild-scoped `/tool_ban` proposals, votes, and status requests.
pub(crate) async fn handle_tool_ban_interaction(
    permissions: &ToolPermissions,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
    guild_id: Option<u64>,
) -> String {
    let Some(guild_id) = guild_id else {
        return "Tool-ban voting is only available inside a server.".into();
    };
    let Some(command) = options.first() else {
        return "Choose `propose`, `vote`, or `status`.".into();
    };
    match command.name.as_str() {
        "propose" => {
            let CommandDataOptionValue::SubCommand(options) = &command.value else {
                return "Unexpected option structure.".into();
            };
            let target = options
                .iter()
                .find(|option| option.name == "user")
                .and_then(|option| match option.value {
                    CommandDataOptionValue::User(user) => Some(user.get()),
                    _ => None,
                });
            let tool = options
                .iter()
                .find(|option| option.name == "tool")
                .and_then(|option| match &option.value {
                    CommandDataOptionValue::String(tool) => Some(tool.as_str()),
                    _ => None,
                });
            let (Some(target), Some(tool)) = (target, tool) else {
                return "Please specify both a user and tool name.".into();
            };
            match permissions.propose(guild_id, target, tool, author_id).await {
                Ok(proposal) => format!(
                    "🗳️ Proposed banning <@{}> from `{}`. Proposal `{}` is open for 24 hours.\nVote with `/tool_ban vote proposal:{} approve:true|false`. The proposal needs at least {} votes; your approval was recorded automatically.",
                    proposal.target_user_id,
                    proposal.tool_name,
                    &proposal.id[..8],
                    &proposal.id[..8],
                    permissions.min_votes()
                ),
                Err(error) => format!("⚠️ {error}"),
            }
        }
        "vote" => {
            let CommandDataOptionValue::SubCommand(options) = &command.value else {
                return "Unexpected option structure.".into();
            };
            let proposal = options
                .iter()
                .find(|option| option.name == "proposal")
                .and_then(|option| match &option.value {
                    CommandDataOptionValue::String(id) => Some(id.as_str()),
                    _ => None,
                });
            let approve = options
                .iter()
                .find(|option| option.name == "approve")
                .and_then(|option| match option.value {
                    CommandDataOptionValue::Boolean(approve) => Some(approve),
                    _ => None,
                });
            let (Some(proposal), Some(approve)) = (proposal, approve) else {
                return "Please specify a proposal ID and vote.".into();
            };
            match permissions.vote(guild_id, proposal, author_id, approve).await {
                Ok(VoteResult::Pending {
                    approvals,
                    rejections,
                    quorum,
                }) => format!(
                    "✅ Vote recorded. Current result: **{approvals} approve / {rejections} reject** (minimum {quorum} votes)."
                ),
                Ok(VoteResult::Approved(ban)) => format!(
                    "🚫 Vote passed. <@{}> is now blocked from using `{}` in this server.",
                    ban.user_id, ban.tool_name
                ),
                Ok(VoteResult::Rejected) => {
                    "✅ The proposal was rejected by majority vote.".into()
                }
                Ok(VoteResult::RestoreVoted(_)) => {
                    "⚠️ Unexpected result from ban vote.".into()
                }
                Err(error) => format!("⚠️ {error}"),
            }
        }
        "status" => {
            let status = match permissions.status(guild_id).await {
                Ok(status) => status,
                Err(error) => {
                    tracing::error!(%error, %guild_id, "failed to load tool permission status");
                    return "⚠️ Tool permission status is temporarily unavailable.".into();
                }
            };
            if status.bans.is_empty() && status.proposals.is_empty() {
                return "No active tool bans or open proposals in this server.".into();
            }
            let mut lines = vec!["**Tool permissions**".to_string()];
            if !status.bans.is_empty() {
                lines.push("**Active bans**".into());
                for ban in status.bans.iter().take(10) {
                    lines.push(format!("• <@{}> — `{}`", ban.user_id, ban.tool_name));
                }
            }
            if !status.proposals.is_empty() {
                lines.push("**Open proposals**".into());
                for proposal in status.proposals.iter().take(10) {
                    let (approvals, rejections) = proposal.vote_counts();
                    lines.push(format!(
                        "• `{}`: <@{}> / `{}` — {approvals} approve, {rejections} reject",
                        &proposal.id[..8],
                        proposal.target_user_id,
                        proposal.tool_name
                    ));
                }
            }
            lines.join("\n")
        }
        other => format!("Unknown tool-ban option `{other}`."),
    }
}

/// Handle guild-scoped `/tool_restore` proposals, votes, and status requests.
pub(crate) async fn handle_tool_restore_interaction(
    permissions: &ToolPermissions,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
    guild_id: Option<u64>,
) -> String {
    let Some(guild_id) = guild_id else {
        return "Tool-restore voting is only available inside a server.".into();
    };
    let Some(command) = options.first() else {
        return "Choose `propose`, `vote`, or `status`.".into();
    };
    match command.name.as_str() {
        "propose" => {
            let CommandDataOptionValue::SubCommand(options) = &command.value else {
                return "Unexpected option structure.".into();
            };
            let target = options
                .iter()
                .find(|option| option.name == "user")
                .and_then(|option| match option.value {
                    CommandDataOptionValue::User(user) => Some(user.get()),
                    _ => None,
                });
            let tool = options
                .iter()
                .find(|option| option.name == "tool")
                .and_then(|option| match &option.value {
                    CommandDataOptionValue::String(tool) => Some(tool.as_str()),
                    _ => None,
                });
            let (Some(target), Some(tool)) = (target, tool) else {
                return "Please specify both a user and tool name.".into();
            };
            match permissions.propose_restore(guild_id, target, tool, author_id).await {
                Ok(proposal) => format!(
                    "🗳️ Proposed restoring `{}` access for <@{}>. Proposal `{}` is open for 24 hours.\nVote with `/tool_restore vote proposal:{} approve:true|false`. The proposal needs at least {} votes; your approval was recorded automatically.",
                    proposal.tool_name,
                    proposal.target_user_id,
                    &proposal.id[..8],
                    &proposal.id[..8],
                    permissions.min_votes()
                ),
                Err(error) => format!("⚠️ {error}"),
            }
        }
        "vote" => {
            let CommandDataOptionValue::SubCommand(options) = &command.value else {
                return "Unexpected option structure.".into();
            };
            let proposal = options
                .iter()
                .find(|option| option.name == "proposal")
                .and_then(|option| match &option.value {
                    CommandDataOptionValue::String(id) => Some(id.as_str()),
                    _ => None,
                });
            let approve = options
                .iter()
                .find(|option| option.name == "approve")
                .and_then(|option| match option.value {
                    CommandDataOptionValue::Boolean(approve) => Some(approve),
                    _ => None,
                });
            let (Some(proposal), Some(approve)) = (proposal, approve) else {
                return "Please specify a proposal ID and vote.".into();
            };
            match permissions.vote_restore(guild_id, proposal, author_id, approve).await {
                Ok(VoteResult::Pending {
                    approvals,
                    rejections,
                    quorum,
                }) => format!(
                    "✅ Vote recorded. Current result: **{approvals} approve / {rejections} reject** (minimum {quorum} votes)."
                ),
                Ok(VoteResult::RestoreVoted(ban)) => format!(
                    "✅ Vote passed. <@{}>'s access to `{}` has been restored.",
                    ban.user_id, ban.tool_name
                ),
                Ok(VoteResult::Rejected) => {
                    "✅ The proposal was rejected by majority vote.".into()
                }
                Ok(VoteResult::Approved(_)) => {
                    "⚠️ Unexpected result from restore vote.".into()
                }
                Err(error) => format!("⚠️ {error}"),
            }
        }
        "status" => {
            let status = match permissions.status(guild_id).await {
                Ok(status) => status,
                Err(error) => {
                    tracing::error!(%error, %guild_id, "failed to load tool permission status");
                    return "⚠️ Tool permission status is temporarily unavailable.".into();
                }
            };
            if status.bans.is_empty()
                && status.proposals.is_empty()
                && status.restore_proposals.is_empty()
            {
                return "No active tool bans or open proposals in this server.".into();
            }
            let mut lines = vec!["**Tool permissions**".to_string()];
            if !status.bans.is_empty() {
                lines.push("**Active bans**".into());
                for ban in status.bans.iter().take(10) {
                    lines.push(format!("• <@{}> — `{}`", ban.user_id, ban.tool_name));
                }
            }
            if !status.proposals.is_empty() {
                lines.push("**Open ban proposals**".into());
                for proposal in status.proposals.iter().take(10) {
                    let (approvals, rejections) = proposal.vote_counts();
                    lines.push(format!(
                        "• `{}`: <@{}> / `{}` — {approvals} approve, {rejections} reject",
                        &proposal.id[..8],
                        proposal.target_user_id,
                        proposal.tool_name
                    ));
                }
            }
            if !status.restore_proposals.is_empty() {
                lines.push("**Open restore proposals**".into());
                for p in status.restore_proposals.iter().take(10) {
                    let (approvals, rejections) = p.vote_counts();
                    lines.push(format!(
                        "• `{}`: <@{}> / `{}` — {approvals} approve, {rejections} reject",
                        &p.id[..8],
                        p.target_user_id,
                        p.tool_name
                    ));
                }
            }
            lines.join("\n")
        }
        other => format!("Unknown tool-restore option `{other}`."),
    }
}

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
    format!(
        "**Your current settings:**\n• Effort level: {effort}\n• Follow-up replies: {followup}\n• Personality: {personality}\n\nUse `/effort` to change the thinking effort level."
    )
}

pub(crate) async fn handle_labs_interaction(
    user_cfg: &UserConfigStore,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
) -> String {
    let mut cfg = user_cfg.load(author_id).await;
    let Some(top) = options.first() else {
        return "Choose a labs feature. Use `/labs list` to see available features.".into();
    };
    match top.name.as_str() {
        "list" => format!(
            "**Labs features**\n• Pagination: {}",
            if cfg.labs_pagination_enabled {
                "enabled"
            } else {
                "disabled"
            }
        ),
        "pagination" => {
            let CommandDataOptionValue::SubCommand(sub_opts) = &top.value else {
                return "Unexpected option structure.".into();
            };
            let Some(enabled) =
                sub_opts
                    .iter()
                    .find(|o| o.name == "enabled")
                    .and_then(|o| match &o.value {
                        CommandDataOptionValue::Boolean(value) => Some(*value),
                        _ => None,
                    })
            else {
                return "Please specify `enabled`.".into();
            };
            cfg.labs_pagination_enabled = enabled;
            if let Err(error) = user_cfg.save(author_id, &cfg).await {
                tracing::error!(target: "housebot::labs::pagination", user_id = author_id, %error, "Failed to save pagination setting");
                return "Error: failed to save labs configuration.".into();
            }
            tracing::info!(target: "housebot::labs::pagination", user_id = author_id, enabled, "Updated pagination setting");
            format!(
                "✅ Paginated responses {}.",
                if enabled { "enabled" } else { "disabled" }
            )
        }
        other => format!("Unknown labs feature `{other}`. Use `/labs list`."),
    }
}

/// Handle `/data profile`: show or clear profile data.
pub(crate) async fn handle_profile_interaction(
    profile_store: &ProfileStore,
    memory: &Memory,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
    guild_id: Option<u64>,
) -> String {
    let profile = profile_store.load(author_id).await;
    let subcommand = options.first().map(|o| o.name.as_str());
    match subcommand {
        Some("clear") => {
            let mut profile = profile_store.load(author_id).await;
            profile.clear_learned();
            let profile_result = profile_store.save(author_id, &profile).await;
            let memory_result = memory.clear(author_id.to_string()).await;
            if profile_result.is_err() || memory_result.is_err() {
                "⚠️ Could not clear all learned profile data.".into()
            } else {
                "✅ Profile learned data and memory cleared. Your Discord identity is preserved."
                    .into()
            }
        }
        _ => {
            let name = profile.best_name();
            let tags: Vec<String> = profile
                .tags
                .iter()
                .map(|t| t.as_str().to_string())
                .collect();
            let actions = profile.quick_actions();
            let mut lines = vec![
                format!("**Profile for {name}**"),
                format!("Username: {}", profile.username),
                format!("Display name: {}", profile.display_name),
                format!(
                    "Guild: {}",
                    guild_id
                        .map(|g| g.to_string())
                        .unwrap_or_else(|| "DM".to_string())
                ),
            ];
            if !profile.nickname.is_empty() {
                lines.push(format!("Nickname: {}", profile.nickname));
            }
            if !profile.avatar_url.is_empty() {
                lines.push("Avatar: (set)".to_string());
            }
            if !tags.is_empty() {
                lines.push(format!("Tags: {}", tags.join(", ")));
            }
            if !actions.is_empty() {
                let action_strs: Vec<String> =
                    actions.iter().map(|(k, v)| format!("{k}: {v}")).collect();
                lines.push(format!("Quick actions: {}", action_strs.join(", ")));
            }
            lines.join("\n")
        }
    }
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
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
) -> String {
    let Some(command) = options.first() else {
        return "Usage: `/skill list` | `/skill info <name>` | `/skill add <name> <prompt>` | `/skill delete <name>`".into();
    };
    let sub_opts = match &command.value {
        CommandDataOptionValue::SubCommand(opts) => opts,
        _ => return "Unexpected option structure.".into(),
    };
    match command.name.as_str() {
        "list" => skill_command(skills, "!skill list", "", author_id).await,
        "info" => {
            let name = sub_opts
                .iter()
                .find(|o| o.name == "name")
                .and_then(|o| match &o.value {
                    CommandDataOptionValue::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            skill_command(skills, &format!("!skill info {name}"), "", author_id).await
        }
        "add" => {
            let name = sub_opts
                .iter()
                .find(|o| o.name == "name")
                .and_then(|o| match &o.value {
                    CommandDataOptionValue::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let prompt = sub_opts
                .iter()
                .find(|o| o.name == "prompt")
                .and_then(|o| match &o.value {
                    CommandDataOptionValue::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            skill_command(skills, &format!("!skill add {name}"), &prompt, author_id).await
        }
        "delete" => {
            let name = sub_opts
                .iter()
                .find(|o| o.name == "name")
                .and_then(|o| match &o.value {
                    CommandDataOptionValue::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            skill_command(skills, &format!("!skill delete {name}"), "", author_id).await
        }
        other => format!("Unknown subcommand `{other}`. Options: list, info, add, delete"),
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
