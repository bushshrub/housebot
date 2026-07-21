//! The /config, /server-config, and /personalize slash-command handlers.

use super::*;

const NOT_CONFIGURER: &str = "Only users authorized to configure the bot can change this setting. \
     Ask the bot owner for access via `/config access allow`.";

const NOT_SERVER_ADMIN: &str =
    "Only server administrators and users authorized to configure the bot can change this setting.";

/// The /config handler: deployment-wide bot configuration, configurers only.
pub(crate) async fn handle_config_interaction(
    access_store: &AccessControlStore,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
) -> String {
    let Some(top) = options.first() else {
        return "No subcommand provided.".into();
    };
    let access = access_store.load().await;
    if !access.is_configurer(author_id, config::owner_id()) {
        return NOT_CONFIGURER.into();
    }

    match top.name.as_str() {
        "proactive" => {
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
            let Some(enabled) = enabled else {
                return "Please specify `enabled`.".into();
            };
            if access_store
                .update(|access| access.proactive_enabled = enabled)
                .await
                .is_err()
            {
                return "Error: failed to save config.".into();
            }
            if enabled {
                "✅ Proactive assistance is enabled again; server and personal settings apply."
                    .into()
            } else {
                "✅ Proactive assistance is now disabled for everyone, regardless of server or personal settings.".into()
            }
        }

        "access" => {
            let sub_opts = match &top.value {
                CommandDataOptionValue::SubCommandGroup(opts) => opts,
                _ => return "Unexpected option structure.".into(),
            };
            let Some(sub) = sub_opts.first() else {
                return "No access subcommand provided.".into();
            };
            match sub.name.as_str() {
                "list" => {
                    let owner = match config::owner_id() {
                        0 => "not configured".to_string(),
                        id => format!("<@{id}>"),
                    };
                    if access.configurer_ids.is_empty() {
                        format!("Owner (always allowed): {owner}\nAdditional configurers: none")
                    } else {
                        let mut ids: Vec<_> = access.configurer_ids.iter().collect();
                        ids.sort_unstable();
                        let list = ids
                            .iter()
                            .map(|id| format!("<@{id}>"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("Owner (always allowed): {owner}\nAdditional configurers: {list}")
                    }
                }
                action @ ("allow" | "revoke") => {
                    let options = match &sub.value {
                        CommandDataOptionValue::SubCommand(opts) => opts,
                        _ => return "Unexpected option structure.".into(),
                    };
                    let target = options.iter().find_map(|option| match option.value {
                        CommandDataOptionValue::User(user) if option.name == "user" => {
                            Some(user.get())
                        }
                        _ => None,
                    });
                    let Some(target) = target else {
                        return "Please provide a valid user.".into();
                    };
                    if target == config::owner_id() {
                        return "The bot owner is always allowed to configure the bot.".into();
                    }
                    let changed = access_store
                        .update(|access| {
                            if action == "allow" {
                                access.configurer_ids.insert(target)
                            } else {
                                access.configurer_ids.remove(&target)
                            }
                        })
                        .await;
                    let Ok(changed) = changed else {
                        return "Error: failed to save config.".into();
                    };
                    match (action, changed) {
                        ("allow", true) => format!("✅ <@{target}> can now configure the bot."),
                        ("revoke", true) => {
                            format!("✅ <@{target}> can no longer configure the bot.")
                        }
                        ("allow", false) => {
                            format!("<@{target}> is already allowed to configure the bot.")
                        }
                        _ => format!("<@{target}> was not allowed to configure the bot."),
                    }
                }
                other => format!("Unknown access subcommand `{other}`."),
            }
        }

        "user" => {
            let sub_opts = match &top.value {
                CommandDataOptionValue::SubCommandGroup(opts) => opts,
                _ => return "Unexpected option structure.".into(),
            };
            let Some(sub) = sub_opts.first() else {
                return "No user subcommand provided.".into();
            };
            let options = match &sub.value {
                CommandDataOptionValue::SubCommand(opts) => opts,
                _ => return "Unexpected option structure.".into(),
            };
            let target = options.iter().find_map(|option| match option.value {
                CommandDataOptionValue::User(user) if option.name == "user" => Some(user.get()),
                _ => None,
            });
            let Some(target) = target else {
                return "Please provide a valid user.".into();
            };
            match sub.name.as_str() {
                "show" => {
                    let policy = access.policy(target);
                    let limit = policy
                        .max_output_tokens
                        .map_or("no limit".to_string(), |cap| format!("{cap} tokens"));
                    format!(
                        "<@{target}>: max output {limit}, responds: {}",
                        policy.respond
                    )
                }
                "limit" => {
                    let cap = options.iter().find_map(|option| match option.value {
                        CommandDataOptionValue::Integer(value) if option.name == "max_tokens" => {
                            Some(value)
                        }
                        _ => None,
                    });
                    let cap = match cap {
                        Some(value) if value < 1 => {
                            return "The token limit must be at least 1 (omit it to remove the cap)."
                                .into()
                        }
                        Some(value) => match u32::try_from(value) {
                            Ok(value) => Some(value),
                            Err(_) => {
                                return format!("The token limit must be at most {}.", u32::MAX)
                            }
                        },
                        None => None,
                    };
                    if access_store
                        .update(|access| {
                            access
                                .user_policies
                                .entry(target)
                                .or_default()
                                .max_output_tokens = cap;
                        })
                        .await
                        .is_err()
                    {
                        return "Error: failed to save config.".into();
                    }
                    match cap {
                        Some(cap) => {
                            format!("✅ <@{target}>'s output is now capped at {cap} tokens.")
                        }
                        None => format!("✅ <@{target}>'s output token cap was removed."),
                    }
                }
                "respond" => {
                    let enabled = options.iter().find_map(|option| match option.value {
                        CommandDataOptionValue::Boolean(value) if option.name == "enabled" => {
                            Some(value)
                        }
                        _ => None,
                    });
                    let Some(enabled) = enabled else {
                        return "Please specify `enabled`.".into();
                    };
                    if access_store
                        .update(|access| {
                            access.user_policies.entry(target).or_default().respond = enabled;
                        })
                        .await
                        .is_err()
                    {
                        return "Error: failed to save config.".into();
                    }
                    if enabled {
                        format!("✅ The bot will respond to <@{target}> again.")
                    } else {
                        format!(
                            "✅ The bot will no longer respond to <@{target}>. \
                             Configurers are exempt from this policy."
                        )
                    }
                }
                other => format!("Unknown user subcommand `{other}`."),
            }
        }

        other => format!("Unknown config option `{other}`."),
    }
}

/// The /server-config handler: guild-scoped settings, available to server
/// administrators and bot configurers (`authorized` carries that check).
pub(crate) async fn handle_server_config_interaction(
    server_cfg: &ServerConfigStore,
    options: &[serenity::all::CommandDataOption],
    guild_id: Option<u64>,
    authorized: bool,
) -> String {
    let Some(top) = options.first() else {
        return "No subcommand provided.".into();
    };
    let Some(gid) = guild_id else {
        return "Server configuration is only available in servers, not DMs.".into();
    };
    if !authorized {
        return NOT_SERVER_ADMIN.into();
    }

    match top.name.as_str() {
        "leaderboard" => {
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

        "bot_pings" => {
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
            let Some(enabled) = enabled else {
                return "Please specify `enabled`.".into();
            };
            let mut cfg = server_cfg.load(gid).await;
            cfg.respond_to_bot_pings = enabled;
            if server_cfg.save(gid, &cfg).await.is_err() {
                return "Error: failed to save config.".into();
            }
            format!(
                "✅ Responses to other bots' pings {}.",
                if enabled { "enabled" } else { "disabled" }
            )
        }

        "proactive" => {
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
            let Some(enabled) = enabled else {
                return "Please specify `enabled`.".into();
            };
            let mut cfg = server_cfg.load(gid).await;
            cfg.proactive_allowed = enabled;
            if server_cfg.save(gid, &cfg).await.is_err() {
                return "Error: failed to save config.".into();
            }
            if enabled {
                "✅ Proactive assistance is allowed in this server; users still opt in via `/personalize proactive`.".into()
            } else {
                "✅ Proactive assistance is disabled in this server for everyone.".into()
            }
        }

        "embeds" => {
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
            let Some(enabled) = enabled else {
                return "Please specify `enabled`.".into();
            };
            let mut cfg = server_cfg.load(gid).await;
            cfg.render_embeds = enabled;
            if server_cfg.save(gid, &cfg).await.is_err() {
                return "Error: failed to save config.".into();
            }
            if enabled {
                "✅ Embed-rendered responses are enabled in this server; users still control pagination individually via `/labs pagination`.".into()
            } else {
                "✅ Embed-rendered responses are disabled in this server; all bot responses will use plain text.".into()
            }
        }

        other => format!("Unknown server-config option `{other}`."),
    }
}

/// The /personalize slash-command handler: per-user settings any user may change.
pub(crate) async fn handle_personalize_interaction(
    user_cfg: &UserConfigStore,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
    can_manage_other_users: bool,
) -> String {
    let Some(top) = options.first() else {
        return "No subcommand provided.".into();
    };
    let sub_opts = match &top.value {
        CommandDataOptionValue::SubCommand(opts) => opts,
        _ => return "Unexpected option structure.".into(),
    };
    let target_id = sub_opts
        .iter()
        .find_map(|option| match option.value {
            CommandDataOptionValue::User(user) if option.name == "user" => Some(user.get()),
            _ => None,
        })
        .unwrap_or(author_id);
    if target_id != author_id && !can_manage_other_users {
        return "Only server administrators and bot configurers can configure another user's settings.".into();
    }
    let mut cfg = user_cfg.load(target_id).await;

    match top.name.as_str() {
        "personality" => {
            let text = sub_opts
                .iter()
                .find(|o| o.name == "text")
                .and_then(|o| match &o.value {
                    CommandDataOptionValue::String(s) => Some(s.clone()),
                    _ => None,
                })
                .filter(|s| !s.trim().is_empty());
            cfg.personality = text.clone();
            if user_cfg.save(target_id, &cfg).await.is_err() {
                return "Error: failed to save config.".into();
            }
            match text {
                None => "✅ Personality cleared — I'll use my default behaviour.".into(),
                Some(s) => format!("✅ Personality set:\n> {}", s.replace('\n', "\n> ")),
            }
        }

        "followup" => {
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
            cfg.followup_enabled = enabled;
            if let Some(secs) = timeout {
                if secs < 1 {
                    return "Timeout must be at least 1 second.".into();
                }
                cfg.followup_timeout_secs = secs as u64;
            }
            if user_cfg.save(target_id, &cfg).await.is_err() {
                return "Error: failed to save config.".into();
            }
            let status = if enabled { "enabled" } else { "disabled" };
            format!(
                "✅ Follow-up replies {status} (timeout: {}s).",
                cfg.followup_timeout_secs
            )
        }

        "proactive" => {
            let enabled =
                sub_opts
                    .iter()
                    .find(|o| o.name == "enabled")
                    .and_then(|o| match &o.value {
                        CommandDataOptionValue::Boolean(b) => Some(*b),
                        _ => None,
                    });
            let Some(enabled) = enabled else {
                return "Please specify `enabled`.".into();
            };
            cfg.proactive_assistance_enabled = enabled;
            if user_cfg.save(target_id, &cfg).await.is_err() {
                return "Error: failed to save config.".into();
            }
            if enabled {
                "✅ Proactive assistance enabled — I may chime in on obvious reminder requests and help questions. Server admins and bot configurers can disable this server-wide or globally.".into()
            } else {
                "✅ Proactive assistance disabled — I'll only respond when addressed.".into()
            }
        }

        "progress" => {
            let enabled = sub_opts.iter().find_map(|option| match option.value {
                CommandDataOptionValue::Boolean(value) if option.name == "enabled" => Some(value),
                _ => None,
            });
            let Some(enabled) = enabled else {
                return "Please specify `enabled`.".into();
            };
            cfg.progress_updates_enabled = enabled;
            if user_cfg.save(target_id, &cfg).await.is_err() {
                return "Error: failed to save config.".into();
            }
            let target = if target_id == author_id {
                "Your".to_string()
            } else {
                format!("User `{target_id}`'s")
            };
            if enabled {
                format!("✅ {target} progress updates are enabled.")
            } else {
                format!(
                    "✅ {target} progress updates are disabled; only final responses will be sent."
                )
            }
        }

        other => format!("Unknown personalize option `{other}`."),
    }
}
