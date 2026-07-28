//! `/personalize` interaction handling.

//! The /config, /server-config, and /personalize slash-command handlers.

use super::*;

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
