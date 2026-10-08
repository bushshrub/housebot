//! Slash-command interaction handlers (effort, status, labs, data, skill, stats).

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
                format!("• **{mode}** — {}{marker}", mode.description())
            })
            .collect();
        return format!(
            "**{whose} thinking effort:** currently **{}** ({}).\n{}\nUse `/effort level:<mode>` to change it.",
            cfg.thinking_mode,
            cfg.thinking_mode.description(),
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
            mode.description()
        )
    } else {
        format!(
            "✅ Thinking effort for user `{target_id}` set to **{mode}** ({}).",
            mode.description()
        )
    }
}

/// Handle `/status`: the caller's own effort and personality settings.
pub(crate) async fn handle_status_interaction(
    user_cfg: &UserConfigStore,
    author_id: u64,
) -> String {
    let cfg = user_cfg.load(author_id).await;
    let effort = format!(
        "**{}** — {}",
        cfg.thinking_mode,
        cfg.thinking_mode.description()
    );
    let personality = match &cfg.personality {
        Some(p) if !p.trim().is_empty() => format!("> {}", p.trim().replace('\n', "\n> ")),
        _ => "default".to_string(),
    };
    format!(
        "**Your current settings:**\n• Effort level: {effort}\n• Personality: {personality}\n\nUse `/effort` to change the thinking effort level."
    )
}

/// Who is running a `/labs` command, for the per-subcommand permission checks.
pub(crate) struct LabsCaller {
    pub(crate) guild_id: Option<u64>,
    pub(crate) is_server_admin: bool,
    pub(crate) is_configurer: bool,
}

pub(crate) async fn handle_labs_interaction(
    agent: &Agent,
    server_cfg: &ServerConfigStore,
    options: &[serenity::all::CommandDataOption],
    caller: LabsCaller,
) -> String {
    let Some(top) = options.first() else {
        return "Choose a labs feature. Use `/labs list` to see available features.".into();
    };
    let sub_opts = match &top.value {
        CommandDataOptionValue::SubCommand(opts) => opts.as_slice(),
        _ => &[],
    };
    match top.name.as_str() {
        "list" => {
            let classifier = match agent.classifier_settings() {
                Some(settings) => format!("`{}` at <{}>", settings.model, settings.url),
                None => "off".to_string(),
            };
            let proactive = match caller.guild_id {
                Some(gid) => {
                    let mut ids: Vec<_> = server_cfg
                        .load(gid)
                        .await
                        .proactive_channel_ids
                        .into_iter()
                        .collect();
                    ids.sort_unstable();
                    if ids.is_empty() {
                        "no channels".to_string()
                    } else {
                        ids.iter()
                            .map(|id| format!("<#{id}>"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                }
                None => "only available in servers".to_string(),
            };
            format!("**Labs features**\n• Classifier: {classifier}\n• Proactive mode: {proactive}")
        }
        "proactive" => {
            let Some(gid) = caller.guild_id else {
                return "Proactive mode is only available in servers, not DMs.".into();
            };
            if !(caller.is_server_admin || caller.is_configurer) {
                return "Only server administrators and users authorized to configure the bot can change this setting.".into();
            }
            let channel_id = sub_opts.iter().find_map(|option| match option.value {
                CommandDataOptionValue::Channel(channel) if option.name == "channel" => {
                    Some(channel.get())
                }
                _ => None,
            });
            let enabled = sub_opts.iter().find_map(|option| match option.value {
                CommandDataOptionValue::Boolean(value) if option.name == "enabled" => Some(value),
                _ => None,
            });
            let (Some(channel_id), Some(enabled)) = (channel_id, enabled) else {
                return "Please specify `channel` and `enabled`.".into();
            };
            let mut cfg = server_cfg.load(gid).await;
            if enabled {
                cfg.proactive_channel_ids.insert(channel_id);
            } else {
                cfg.proactive_channel_ids.remove(&channel_id);
            }
            if server_cfg.save(gid, &cfg).await.is_err() {
                return "Error: failed to save config.".into();
            }
            match (enabled, agent.classifier_settings().is_some()) {
                (false, _) => format!("✅ Proactive mode disabled in <#{channel_id}>."),
                (true, true) => format!("✅ Proactive mode enabled in <#{channel_id}>."),
                (true, false) => format!(
                    "✅ Proactive mode enabled in <#{channel_id}>, but the classifier is off, so nothing will happen until a configurer sets it with `/labs classifier`."
                ),
            }
        }
        "classifier" => {
            if !caller.is_configurer {
                return "Only users authorized to configure the bot can change the classifier."
                    .into();
            }
            let string_option = |name: &str| {
                sub_opts.iter().find_map(|option| match &option.value {
                    CommandDataOptionValue::String(value) if option.name == name => {
                        Some(value.trim().to_string())
                    }
                    _ => None,
                })
            };
            let disable = sub_opts.iter().any(|option| {
                option.name == "disable" && option.value == CommandDataOptionValue::Boolean(true)
            });
            let url = string_option("url");
            let model = string_option("model");
            let current = agent.classifier_settings();
            if disable {
                if agent.set_classifier(None).await.is_err() {
                    return "Error: failed to save config.".into();
                }
                return "✅ Classifier disabled. Pings get a full answer, and proactive mode does nothing.".into();
            }
            if url.is_none() && model.is_none() {
                return match current {
                    Some(settings) => format!(
                        "Classifier: `{}` at <{}>.",
                        settings.model, settings.url
                    ),
                    None => "The classifier is off. Set it with `/labs classifier url:<base URL> model:<name>`.".into(),
                };
            }
            let Some(url) = url.or_else(|| current.as_ref().map(|c| c.url.clone())) else {
                return "Please specify `url` the first time you set the classifier.".into();
            };
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return "The URL must start with `https://` or `http://`.".into();
            }
            let model = model
                .or_else(|| current.map(|c| c.model))
                .unwrap_or_else(|| "kev".to_string());
            let settings = crate::bot_config::ClassifierSettings { url, model };
            if agent.set_classifier(Some(settings.clone())).await.is_err() {
                return "Error: failed to save config.".into();
            }
            format!(
                "✅ Classifier set to `{}` at <{}>.",
                settings.model, settings.url
            )
        }
        other => format!("Unknown labs feature `{other}`. Use `/labs list`."),
    }
}

/// Handle `/data profile`: show or clear profile data.
pub(crate) async fn handle_history_interaction(
    history: &History,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
    display_name: &str,
    _guild_id: Option<u64>,
) -> String {
    let subcommand = options.first().map(|o| o.name.as_str());
    match subcommand {
        Some("clear") => {
            let _ = history.clear(author_id.to_string()).await;
            format!("✅ Conversation history cleared for {display_name}.")
        }
        _ => {
            let hist = history.load(author_id.to_string()).await;
            render_history(display_name, &hist)
        }
    }
}

pub(crate) fn render_history(display_name: &str, hist: &[serde_json::Value]) -> String {
    let mut lines = vec![
        format!("**History for {display_name}**"),
        "Scope: all servers and channels where you used housebot".to_string(),
    ];

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

pub(crate) async fn handle_skill_interaction(
    skills: &Skills,
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
        "list" => skill_list(skills).await,
        "info" => skill_info(skills, &name_option(sub_opts)).await,
        "delete" => skill_delete(skills, author_id, &name_option(sub_opts)).await,
        other => format!("Unknown subcommand `{other}`. Options: list, info, delete"),
    }
}

pub(crate) async fn handle_stats_interaction(
    history: &History,
    memory: &Memory,
    skills: &Skills,
    author_id: u64,
    display_name: &str,
    token_summary: &str,
) -> String {
    let stats = stats_command(history, memory, skills, author_id, display_name).await;
    format!("{stats}\n• Token usage: {token_summary}")
}
