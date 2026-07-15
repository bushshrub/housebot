//! The /config slash-command interaction handler.

use super::*;

pub(crate) async fn handle_config_interaction(
    server_cfg: &ServerConfigStore,
    user_cfg: &UserConfigStore,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
    guild_id: Option<u64>,
    is_admin: bool,
) -> String {
    let Some(top) = options.first() else {
        return "No subcommand provided.".into();
    };

    match top.name.as_str() {
        "leaderboard" => {
            let Some(gid) = guild_id else {
                return "Leaderboard configuration is only available in servers.".into();
            };
            if !is_admin {
                return "Only server administrators can configure leaderboard visibility.".into();
            }
            let sub_opts = match &top.value {
                CommandDataOptionValue::SubCommandGroup(options) => options,
                _ => return "Unexpected option structure.".into(),
            };
            let Some(sub) = sub_opts.first() else {
                return "No leaderboard subcommand provided.".into();
            };
            let mut cfg = server_cfg.load(gid).await;
            match sub.name.as_str() {
                "visibility" => {
                    let options = match &sub.value {
                        CommandDataOptionValue::SubCommand(options) => options,
                        _ => return "Unexpected option structure.".into(),
                    };
                    let visibility = options.iter().find_map(|option| match &option.value {
                        CommandDataOptionValue::String(value) if option.name == "mode" => {
                            Some(value.as_str())
                        }
                        _ => None,
                    });
                    cfg.leaderboard_visibility = match visibility {
                        Some("public") => LeaderboardVisibility::Public,
                        Some("private") => LeaderboardVisibility::Private,
                        Some("restricted") => LeaderboardVisibility::Restricted,
                        _ => return "Please choose a valid visibility mode.".into(),
                    };
                    if server_cfg.save(gid, &cfg).await.is_err() {
                        return "Error: failed to save config.".into();
                    }
                    format!(
                        "✅ Token leaderboard visibility set to **{}**.",
                        cfg.leaderboard_visibility.as_str()
                    )
                }
                action @ ("role_add" | "role_remove") => {
                    let options = match &sub.value {
                        CommandDataOptionValue::SubCommand(options) => options,
                        _ => return "Unexpected option structure.".into(),
                    };
                    let role_id = options.iter().find_map(|option| match option.value {
                        CommandDataOptionValue::Role(role) if option.name == "role" => {
                            Some(role.get())
                        }
                        _ => None,
                    });
                    let Some(role_id) = role_id else {
                        return "Please provide a valid role.".into();
                    };
                    let changed = if action == "role_add" {
                        cfg.leaderboard_role_ids.insert(role_id)
                    } else {
                        cfg.leaderboard_role_ids.remove(&role_id)
                    };
                    if server_cfg.save(gid, &cfg).await.is_err() {
                        return "Error: failed to save config.".into();
                    }
                    match (action, changed) {
                        ("role_add", true) => format!("✅ <@&{role_id}> can view the leaderboard."),
                        ("role_remove", true) => {
                            format!("✅ <@&{role_id}> removed from leaderboard access.")
                        }
                        ("role_add", false) => {
                            format!("<@&{role_id}> already has leaderboard access.")
                        }
                        _ => format!("<@&{role_id}> did not have leaderboard access."),
                    }
                }
                "role_list" => {
                    if cfg.leaderboard_role_ids.is_empty() {
                        "No roles are allowed in restricted mode. Administrators retain access."
                            .into()
                    } else {
                        let roles = cfg
                            .leaderboard_role_ids
                            .iter()
                            .map(|role| format!("<@&{role}>"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("Leaderboard roles: {roles}")
                    }
                }
                other => format!("Unknown leaderboard subcommand `{other}`."),
            }
        }

        "channel" => {
            let Some(gid) = guild_id else {
                return "Channel configuration is only available in servers, not DMs.".into();
            };
            let sub_opts = match &top.value {
                CommandDataOptionValue::SubCommandGroup(opts) => opts,
                _ => return "Unexpected option structure.".into(),
            };
            let Some(sub) = sub_opts.first() else {
                return "No channel subcommand provided.".into();
            };
            match sub.name.as_str() {
                "list" => {
                    let cfg = server_cfg.load(gid).await;
                    if cfg.allowed_channel_ids.is_empty() {
                        "I'm allowed to respond in **all channels** (no restriction set). Follow-up replies are disabled until you add explicit reply channels.".into()
                    } else {
                        let ids: Vec<String> = cfg
                            .allowed_channel_ids
                            .iter()
                            .map(|id| format!("<#{id}>"))
                            .collect();
                        format!("Allowed channels: {}", ids.join(", "))
                    }
                }
                "clear" => {
                    let mut cfg = server_cfg.load(gid).await;
                    cfg.allowed_channel_ids.clear();
                    if server_cfg.save(gid, &cfg).await.is_err() {
                        return "Error: failed to save config.".into();
                    }
                    "✅ Channel restriction cleared — I'll respond in all channels, but follow-up replies are disabled until you add explicit reply channels.".into()
                }
                action @ ("add" | "remove") => {
                    let channel_opts = match &sub.value {
                        CommandDataOptionValue::SubCommand(opts) => opts,
                        _ => return "Unexpected option structure.".into(),
                    };
                    let channel_id =
                        channel_opts
                            .iter()
                            .find(|o| o.name == "channel")
                            .and_then(|o| match &o.value {
                                CommandDataOptionValue::Channel(c) => Some(c.get()),
                                _ => None,
                            });
                    let Some(cid) = channel_id else {
                        return "Please provide a valid channel.".into();
                    };
                    let mut cfg = server_cfg.load(gid).await;
                    if action == "add" {
                        cfg.allowed_channel_ids.insert(cid);
                        if server_cfg.save(gid, &cfg).await.is_err() {
                            return "Error: failed to save config.".into();
                        }
                        format!("✅ <#{cid}> added to the allowlist.")
                    } else {
                        let removed = cfg.allowed_channel_ids.remove(&cid);
                        if server_cfg.save(gid, &cfg).await.is_err() {
                            return "Error: failed to save config.".into();
                        }
                        if removed {
                            format!("✅ <#{cid}> removed from the allowlist.")
                        } else {
                            format!("<#{cid}> was not in the allowlist.")
                        }
                    }
                }
                other => format!("Unknown channel subcommand `{other}`."),
            }
        }

        "personality" => {
            let sub_opts = match &top.value {
                CommandDataOptionValue::SubCommand(opts) => opts,
                _ => return "Unexpected option structure.".into(),
            };
            let text = sub_opts
                .iter()
                .find(|o| o.name == "text")
                .and_then(|o| match &o.value {
                    CommandDataOptionValue::String(s) => Some(s.clone()),
                    _ => None,
                });
            let mut cfg = user_cfg.load(author_id).await;
            match text {
                None => {
                    cfg.personality = None;
                    if user_cfg.save(author_id, &cfg).await.is_err() {
                        return "Error: failed to save config.".into();
                    }
                    "✅ Personality cleared — I'll use my default behaviour.".into()
                }
                Some(ref s) if s.trim().is_empty() => {
                    cfg.personality = None;
                    if user_cfg.save(author_id, &cfg).await.is_err() {
                        return "Error: failed to save config.".into();
                    }
                    "✅ Personality cleared — I'll use my default behaviour.".into()
                }
                Some(s) => {
                    cfg.personality = Some(s.clone());
                    if user_cfg.save(author_id, &cfg).await.is_err() {
                        return "Error: failed to save config.".into();
                    }
                    format!("✅ Personality set:\n> {}", s.replace('\n', "\n> "))
                }
            }
        }

        "followup" => {
            let sub_opts = match &top.value {
                CommandDataOptionValue::SubCommand(opts) => opts,
                _ => return "Unexpected option structure.".into(),
            };
            let enabled =
                sub_opts
                    .iter()
                    .find(|o| o.name == "enabled")
                    .and_then(|o| match &o.value {
                        CommandDataOptionValue::Boolean(b) => Some(*b),
                        _ => None,
                    });
            let timeout =
                sub_opts
                    .iter()
                    .find(|o| o.name == "timeout")
                    .and_then(|o| match &o.value {
                        CommandDataOptionValue::Integer(n) => Some(*n),
                        _ => None,
                    });
            let Some(enabled) = enabled else {
                return "Please specify `enabled`.".into();
            };
            let mut cfg = user_cfg.load(author_id).await;
            cfg.followup_enabled = enabled;
            if let Some(secs) = timeout {
                if secs < 1 {
                    return "Timeout must be at least 1 second.".into();
                }
                cfg.followup_timeout_secs = secs as u64;
            }
            if user_cfg.save(author_id, &cfg).await.is_err() {
                return "Error: failed to save config.".into();
            }
            let status = if enabled { "enabled" } else { "disabled" };
            format!(
                "✅ Follow-up replies {status} (timeout: {}s).",
                cfg.followup_timeout_secs
            )
        }

        other => format!("Unknown config option `{other}`."),
    }
}
