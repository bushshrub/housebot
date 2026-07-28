//! Interactions settings.

//! Slash-command interaction handlers (effort, tool bans, status, data, privacy, skill, stats).

use super::*;

pub(crate) async fn handle_effort_interaction(
    user_cfg: &UserConfigStore,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
    can_manage_other_users: bool,
) -> String {
    let level = options
        .iter()
        .find(|o| o.name == "level")
        .and_then(|o| match &o.value {
            CommandDataOptionValue::String(s) => Some(s.clone()),
            _ => None,
        });
    let target_id = options
        .iter()
        .find(|o| o.name == "user")
        .and_then(|o| match o.value {
            CommandDataOptionValue::User(user) => Some(user.get()),
            _ => None,
        })
        .unwrap_or(author_id);
    if target_id != author_id && !can_manage_other_users {
        return "Only server administrators and bot configurers can configure another user's thinking effort.".into();
    }
    let mut cfg = user_cfg.load(target_id).await;
    let whose = if target_id == author_id {
        "Your".to_string()
    } else {
        format!("User `{target_id}`'s")
    };
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
            "**{whose} thinking effort:** currently **{}** ({}).\n{}\nUse `/effort level:<mode>` to change it.",
            cfg.thinking_mode,
            cfg.thinking_mode.budget_label(),
            lines.join("\n")
        );
    };
    let Ok(mode) = level.parse::<ThinkingMode>() else {
        return format!(
            "Unknown effort level `{level}`. Options: instant, low, medium, high, xhigh, max."
        );
    };
    cfg.thinking_mode = mode;
    if let Err(error) = user_cfg.save(target_id, &cfg).await {
        tracing::error!(target: "housebot::commands", user_id = target_id, changed_by = author_id, %error, "Failed to save effort setting");
        return "Error: failed to save config.".into();
    }
    tracing::info!(target: "housebot::commands", user_id = target_id, changed_by = author_id, mode = %mode, "Thinking effort updated");
    if target_id == author_id {
        format!(
            "✅ Thinking effort set to **{mode}** ({}).",
            mode.budget_label()
        )
    } else {
        format!(
            "✅ Thinking effort for user `{target_id}` set to **{mode}** ({}).",
            mode.budget_label()
        )
    }
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
