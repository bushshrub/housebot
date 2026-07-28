//! `/config` interaction handling and the shared permission messages.

//! The /config, /server-config, and /personalize slash-command handlers.

use super::*;

pub(crate) use super::config_cmd_personalize::*;
pub(crate) use super::config_cmd_server::*;

pub(crate) const NOT_CONFIGURER: &str =
    "Only users authorized to configure the bot can change this setting. \
     Ask the bot owner for access via `/config access allow`.";

pub(crate) const NOT_SERVER_ADMIN: &str =
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

        "dev_notify_channel" => {
            let sub_opts = match &top.value {
                CommandDataOptionValue::SubCommand(opts) => opts,
                _ => return "Unexpected option structure.".into(),
            };
            let channel_id = sub_opts.iter().find_map(|option| match option.value {
                CommandDataOptionValue::Channel(c) if option.name == "channel" => Some(c.get()),
                _ => None,
            });
            if access_store
                .update(|access| access.dev_notify_channel_id = channel_id)
                .await
                .is_err()
            {
                return "Error: failed to save config.".into();
            }
            match channel_id {
                Some(cid) => {
                    format!("✅ Now watching <#{cid}> for feature-development completion notices.")
                }
                None => "✅ Feature-development completion watching disabled.".into(),
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
