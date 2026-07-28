//! The `configure_bot` tool: access control, limits, and per-user settings.

use super::*;

impl Agent {
    pub(super) async fn dispatch_configure_bot(
        &self,
        name: &str,
        args: &Value,
        user_id: &str,
    ) -> Option<ToolOutcome> {
        let outcome = match name {
            // Offered only to configurers at the tool-definition layer, but
            // re-checked here as a defence-in-depth measure.
            "configure_bot" => {
                let caller = user_id.parse::<u64>().unwrap_or(0);
                let access = self.access_control.load().await;
                if !access.is_configurer(caller, config::owner_id()) {
                    return Some(ToolOutcome::Text(
                    "Error: permission denied — only users authorized to configure the bot can use this tool."
                        .into(),
                ));
                }
                ToolOutcome::Text(self.handle_configure_bot(args, access).await)
            }
            // ── Sandbox tools ──
            _ => return None,
        };
        Some(outcome)
    }

    async fn handle_configure_bot(&self, args: &Value, access: AccessControl) -> String {
        let action = str_arg(args, "action");
        if action == "show" {
            let mut lines = vec![format!(
                "Owner (always allowed): {}",
                match config::owner_id() {
                    0 => "not configured".to_string(),
                    id => format!("<@{id}>"),
                }
            )];
            lines.push(format!(
                "Proactive assistance (global): {}",
                if access.proactive_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            ));
            lines.push(format!(
                "Dev notification channel: {}",
                access
                    .dev_notify_channel_id
                    .map(|id| format!("<#{id}>"))
                    .unwrap_or_else(|| "not set".to_string())
            ));
            if access.configurer_ids.is_empty() {
                lines.push("Additional configurers: none".to_string());
            } else {
                let mut ids: Vec<_> = access.configurer_ids.iter().collect();
                ids.sort_unstable();
                lines.push(format!(
                    "Additional configurers: {}",
                    ids.iter()
                        .map(|id| format!("<@{id}>"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if access.user_policies.is_empty() {
                lines.push("User policies: none".to_string());
            } else {
                let mut policies: Vec<_> = access.user_policies.iter().collect();
                policies.sort_unstable_by_key(|(id, _)| **id);
                lines.push(format!("Users with policies: {}", policies.len()));
                for (id, policy) in policies {
                    let limit = policy
                        .max_output_tokens
                        .map_or("no limit".to_string(), |cap| format!("{cap} tokens"));
                    lines.push(format!(
                        "<@{id}>: max output {limit}, responds: {}",
                        policy.respond
                    ));
                }
            }
            return lines.join("\n");
        }

        if action == "set_proactive" {
            let Some(enabled) = args.get("enabled").and_then(Value::as_bool) else {
                return "Error: 'enabled' (true/false) is required for set_proactive.".to_string();
            };
            let updated = self
                .access_control
                .update(|access| {
                    access.proactive_enabled = enabled;
                })
                .await;
            return match updated {
                Ok(_) => {
                    if enabled {
                        "Proactive assistance is now globally **enabled**.".to_string()
                    } else {
                        "Proactive assistance is now globally **disabled**.".to_string()
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "failed to save bot access control");
                    "Error: failed to save the bot configuration.".to_string()
                }
            };
        }

        if action == "set_dev_notify_channel" {
            let channel_id = match optional_nonzero_u64_string(args, "channel_id") {
                Ok(channel_id) => channel_id,
                Err(error) => return error,
            };
            let updated = self
                .access_control
                .update(|access| {
                    access.dev_notify_channel_id = channel_id;
                })
                .await;
            return match updated {
                Ok(_) => match channel_id {
                    Some(id) => format!("Dev notification channel set to <#{id}>."),
                    None => "Dev notification channel disabled.".to_string(),
                },
                Err(error) => {
                    tracing::error!(%error, "failed to save bot access control");
                    "Error: failed to save the bot configuration.".to_string()
                }
            };
        }

        if action == "set_user_limit_all" {
            let cap = match optional_nonzero_u32(args, "max_output_tokens") {
                Ok(cap) => cap,
                Err(error) => return error,
            };
            let updated = self
                .access_control
                .update(|access| {
                    let count = access.user_policies.len();
                    for policy in access.user_policies.values_mut() {
                        policy.max_output_tokens = cap;
                    }
                    count
                })
                .await;
            return match updated {
                Ok(count) => match cap {
                    Some(cap) => {
                        format!(
                            "Set output token cap to {cap} for all {count} user(s) with policies."
                        )
                    }
                    None => {
                        format!("Removed output token caps for all {count} user(s) with policies.")
                    }
                },
                Err(error) => {
                    tracing::error!(%error, "failed to save bot access control");
                    "Error: failed to save the bot configuration.".to_string()
                }
            };
        }

        if action == "set_user_respond_all" {
            let Some(respond) = args.get("respond").and_then(Value::as_bool) else {
                return "Error: 'respond' (true/false) is required for set_user_respond_all."
                    .to_string();
            };
            let updated = self
                .access_control
                .update(|access| {
                    let count = access.user_policies.len();
                    for policy in access.user_policies.values_mut() {
                        policy.respond = respond;
                    }
                    count
                })
                .await;
            return match updated {
                Ok(count) => {
                    if respond {
                        format!("The bot will now respond to all {count} user(s) with policies.")
                    } else {
                        format!(
                            "The bot will no longer respond to all {count} user(s) with policies."
                        )
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "failed to save bot access control");
                    "Error: failed to save the bot configuration.".to_string()
                }
            };
        }

        let target: u64 = str_arg(args, "user_id").parse().unwrap_or(0);
        if target == 0 {
            return "Error: a valid user_id is required for this action.".to_string();
        }
        // Validate inputs first, then apply each change through the store's
        // serialized update so concurrent configuration changes are not lost.
        let updated = match action {
            "allow_configurer" => {
                if target == config::owner_id() {
                    return "The bot owner is always allowed to configure the bot.".to_string();
                }
                self.access_control
                    .update(|access| {
                        if access.configurer_ids.insert(target) {
                            format!("<@{target}> can now configure the bot.")
                        } else {
                            format!("<@{target}> is already allowed to configure the bot.")
                        }
                    })
                    .await
            }
            "revoke_configurer" => {
                if target == config::owner_id() {
                    return "Error: the bot owner is always allowed to configure the bot."
                        .to_string();
                }
                self.access_control
                    .update(|access| {
                        if access.configurer_ids.remove(&target) {
                            format!("<@{target}> can no longer configure the bot.")
                        } else {
                            format!("<@{target}> was not allowed to configure the bot.")
                        }
                    })
                    .await
            }
            "set_user_limit" => {
                let cap = match args
                    .get("max_output_tokens")
                    .and_then(Value::as_u64)
                    .filter(|cap| *cap > 0)
                {
                    None => None,
                    Some(cap) => match u32::try_from(cap) {
                        Ok(cap) => Some(cap),
                        Err(_) => {
                            return format!(
                                "Error: max_output_tokens must be at most {}.",
                                u32::MAX
                            )
                        }
                    },
                };
                self.access_control
                    .update(|access| {
                        access
                            .user_policies
                            .entry(target)
                            .or_default()
                            .max_output_tokens = cap;
                        match cap {
                            Some(cap) => {
                                format!("<@{target}>'s output is now capped at {cap} tokens.")
                            }
                            None => format!("<@{target}>'s output token cap was removed."),
                        }
                    })
                    .await
            }
            "set_user_respond" => {
                let Some(respond) = args.get("respond").and_then(Value::as_bool) else {
                    return "Error: 'respond' (true/false) is required for set_user_respond."
                        .to_string();
                };
                self.access_control
                    .update(|access| {
                        access.user_policies.entry(target).or_default().respond = respond;
                        if respond {
                            format!("The bot will respond to <@{target}> again.")
                        } else {
                            format!("The bot will no longer respond to <@{target}>.")
                        }
                    })
                    .await
            }
            other => return format!("Error: unknown configure_bot action `{other}`."),
        };
        match updated {
            Ok(reply) => reply,
            Err(error) => {
                tracing::error!(%error, "failed to save bot access control");
                "Error: failed to save the bot configuration.".to_string()
            }
        }
    }
}

fn optional_nonzero_u64_string(args: &Value, key: &str) -> Result<Option<u64>, String> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    let Some(value) = value.as_str() else {
        return Err(format!("Error: '{key}' must be a non-zero numeric string."));
    };
    let parsed = value
        .parse::<u64>()
        .ok()
        .filter(|parsed| *parsed > 0)
        .ok_or_else(|| format!("Error: '{key}' must be a non-zero numeric string."))?;
    Ok(Some(parsed))
}

fn optional_nonzero_u32(args: &Value, key: &str) -> Result<Option<u32>, String> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    let parsed = value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("Error: '{key}' must be an integer from 1 to {}.", u32::MAX))?;
    Ok(Some(parsed))
}

#[cfg(test)]
mod configure_bot_argument_tests {
    use super::{optional_nonzero_u32, optional_nonzero_u64_string};
    use serde_json::json;

    #[test]
    fn optional_values_only_clear_when_omitted() {
        assert_eq!(
            optional_nonzero_u64_string(&json!({}), "channel_id"),
            Ok(None)
        );
        assert_eq!(
            optional_nonzero_u32(&json!({}), "max_output_tokens"),
            Ok(None)
        );

        for args in [
            json!({"channel_id": null}),
            json!({"channel_id": ""}),
            json!({"channel_id": "0"}),
            json!({"channel_id": 123}),
        ] {
            assert!(optional_nonzero_u64_string(&args, "channel_id").is_err());
        }
        for args in [
            json!({"max_output_tokens": null}),
            json!({"max_output_tokens": 0}),
            json!({"max_output_tokens": "100"}),
            json!({"max_output_tokens": u64::from(u32::MAX) + 1}),
        ] {
            assert!(optional_nonzero_u32(&args, "max_output_tokens").is_err());
        }
    }
}
