//! Built-in tool dispatch (the large match over tool names).

use super::*;
use crate::discord_bridge::MessageAnchor;

impl Agent {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn dispatch_tool_inner(
        &self,
        name: &str,
        args: &Value,
        user_id: &str,
        username: &str,
        channel_id: u64,
        guild_id: u64,
        sandbox: &LazySandbox,
    ) -> ToolOutcome {
        match name {
            "web_search" => ToolOutcome::Text(
                self.searxng
                    .search(
                        str_arg(args, "query"),
                        u64_arg(args, "max_results", 5) as usize,
                        str_arg(args, "language"),
                    )
                    .await,
            ),
            "fetch_webpage" => ToolOutcome::Text(
                self.web_fetch
                    .fetch_content(str_arg(args, "url"), sandbox)
                    .await,
            ),
            "update_memory" => {
                let new_content = str_arg(args, "memory_content");
                let _ = self.memory.save(user_id, new_content).await;
                ToolOutcome::Text("Memory updated.".to_string())
            }
            "search_memory" => {
                let query = str_arg(args, "query");
                let query = query.trim();
                if query.is_empty() {
                    return ToolOutcome::Text("Error: search query cannot be blank.".to_string());
                }
                let content = self.memory.load(user_id).await;
                if content.trim().is_empty() {
                    ToolOutcome::Text("No memory stored for this user.".to_string())
                } else {
                    let matching = crate::memory::search(&content, query, 10);
                    if matching.is_empty() {
                        ToolOutcome::Text(format!("No memory entries matching '{query}'."))
                    } else {
                        ToolOutcome::Text(matching.join("\n\n"))
                    }
                }
            }
            "github_api" => {
                // Merging is administrator-only; the tools layer re-checks this
                // flag as a defence-in-depth measure and audits every attempt.
                let is_admin = self
                    .access_control
                    .load()
                    .await
                    .is_configurer(user_id.parse::<u64>().unwrap_or(0), config::owner_id());
                let caller = tools::github_api::ToolCaller {
                    user_id,
                    username,
                    is_admin,
                };
                ToolOutcome::Text(
                    tools::github_api::handle_github_api(
                        &self.reporter,
                        str_arg(args, "action"),
                        args,
                        &caller,
                        &self.merge_audit,
                    )
                    .await,
                )
            }
            "create_feature_request" => ToolOutcome::Text(
                tools::feature_request::create_feature_request(
                    &self.reporter,
                    &self.rate_limiter,
                    str_arg(args, "title"),
                    str_arg(args, "description"),
                    str_arg(args, "type"),
                    username,
                    user_id,
                )
                .await,
            ),
            "edit_feature_request" => ToolOutcome::Text(
                tools::edit_feature_request::edit_feature_request(
                    &self.reporter,
                    &self.feature_edit_limiter,
                    u64_arg(args, "issue_number", 0),
                    args.get("title").and_then(Value::as_str),
                    args.get("description").and_then(Value::as_str),
                    user_id,
                )
                .await,
            ),
            "prepare_feature_development" => {
                use crate::coding_agent::pending::{
                    DevelopmentRequester, DiscordMessageRef, PartialAgentSelection,
                };
                use crate::tools::feature_development::{DispatchMode, FeatureDevelopmentOutcome};

                let requirements: Vec<String> = args
                    .get("requirements")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let acceptance_criteria: Vec<String> = args
                    .get("acceptance_criteria")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();

                let owner_id = config::owner_id();
                let requester_user_id: u64 = user_id.parse().unwrap_or(0);
                let issue_number = u64_arg(args, "issue_number", 0);
                if issue_number == 0 {
                    return ToolOutcome::Text(
                        "Error: an existing GitHub issue_number is required.".to_string(),
                    );
                }
                let Some(issue) = self.reporter.fetch_issue(issue_number).await else {
                    return ToolOutcome::Text(format!(
                        "Error: GitHub issue #{issue_number} could not be found in the configured repository."
                    ));
                };
                if issue.pull_request.is_some() {
                    return ToolOutcome::Text(format!(
                        "Error: #{issue_number} is a pull request; feature development requires an existing issue."
                    ));
                }
                let is_configurer = self
                    .access_control
                    .load()
                    .await
                    .is_configurer(requester_user_id, owner_id);
                let dispatch_mode = if is_configurer {
                    DispatchMode::Interactive
                } else {
                    DispatchMode::RequireOwnerApproval
                };

                let requester = DevelopmentRequester {
                    user_id: requester_user_id,
                    username: username.to_string(),
                    channel_id,
                    guild_id: (guild_id != 0).then_some(guild_id),
                    source_message_id: 0,
                };
                let source_message = DiscordMessageRef {
                    channel_id,
                    message_id: 0,
                };

                // Pre-fill defaults so the owner can dispatch immediately without
                // going through the interactive picker. Read from env vars so the
                // operator can override them; fall back to the opencode free tier.
                let defaults = {
                    use crate::coding_agent::catalog::CodingAgent;
                    use std::str::FromStr;
                    let agent_str = config::env_or("DEVELOPMENT_DEFAULT_AGENT", "opencode");
                    let model = config::env_or(
                        "DEVELOPMENT_DEFAULT_MODEL",
                        "opencode/mimo-v2.6-flash-free",
                    );
                    PartialAgentSelection {
                        agent: CodingAgent::from_str(&agent_str).ok(),
                        model: Some(model),
                    }
                };

                let outcome = tools::feature_development::prepare_feature_development(
                    &self.pending_jobs,
                    &self.non_owner_dev_limiter,
                    owner_id,
                    requester,
                    source_message,
                    issue_number,
                    str_arg(args, "title"),
                    str_arg(args, "objective"),
                    str_arg(args, "context"),
                    requirements,
                    acceptance_criteria,
                    dispatch_mode,
                    &defaults,
                );

                let text = outcome.tool_response();
                let action = match &outcome {
                    FeatureDevelopmentOutcome::OwnerConfigurationRequired { job_id } => {
                        Some(AgentControlAction::OwnerConfigurationRequired { job_id: *job_id })
                    }
                    FeatureDevelopmentOutcome::OwnerApprovalRequired { job_id } => {
                        Some(AgentControlAction::OwnerApprovalRequired { job_id: *job_id })
                    }
                    FeatureDevelopmentOutcome::Rejected { .. } => None,
                };
                if let Some(action) = action {
                    ToolOutcome::DevelopmentAction { text, action }
                } else {
                    ToolOutcome::Text(text)
                }
            }
            "set_reminder" => {
                let delay = args
                    .get("delay_minutes")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                ToolOutcome::Text(
                    tools::remind::create_reminder(
                        &self.reminders,
                        user_id,
                        str_arg(args, "message"),
                        delay,
                    )
                    .await,
                )
            }
            "manage_skill" => {
                ToolOutcome::Text(tools::manage_skills::dispatch(&self.skills, user_id, args).await)
            }
            "get_bot_features" => ToolOutcome::Text(tools::features::features_text().to_string()),
            "get_messages" => {
                let mode = args.get("mode").and_then(Value::as_str).unwrap_or("recent");
                let target_channel = args
                    .get("channel_id")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(channel_id);
                if let Err(denial) = self
                    .authorize_channel_read(guild_id, user_id, channel_id, target_channel)
                    .await
                {
                    return ToolOutcome::Text(denial);
                }
                ToolOutcome::Text(match mode {
                    "search" => {
                        let pattern = str_arg(args, "pattern");
                        let limit = u64_arg(args, "limit", 10).clamp(1, 100) as usize;
                        match self.channel_context.search(target_channel, pattern, limit) {
                            Err(e) => format!("Error: {e}"),
                            Ok(msgs) if msgs.is_empty() => {
                                "No matching messages found.".to_string()
                            }
                            Ok(msgs) => msgs
                                .iter()
                                .map(|m| {
                                    let author = m.nick.as_deref().unwrap_or(&m.username);
                                    format!("[{}] {}: {}", m.at.to_rfc3339(), author, m.content)
                                })
                                .collect::<Vec<_>>()
                                .join("\n"),
                        }
                    }
                    "before" | "after" | "around" => {
                        let message_id: Option<u64> = str_arg(args, "message_id")
                            .parse()
                            .ok()
                            .filter(|&id| id != 0);
                        match message_id {
                            None => "Error: invalid message_id.".to_string(),
                            Some(message_id) => {
                                let anchor = match mode {
                                    "before" => MessageAnchor::Before(message_id),
                                    "after" => MessageAnchor::After(message_id),
                                    _ => MessageAnchor::Around(message_id),
                                };
                                let limit = u64_arg(args, "limit", 20).clamp(1, 100) as u8;
                                match self
                                    .discord
                                    .fetch_messages(target_channel, anchor, limit)
                                    .await
                                {
                                    Err(e) => format!("Error: {e}"),
                                    Ok(msgs) if msgs.is_empty() => "No messages found.".to_string(),
                                    Ok(msgs) => msgs
                                        .iter()
                                        .map(|m| {
                                            format!(
                                                "[{}] [{}] {}: {}",
                                                m.id, m.ts, m.author, m.content
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                }
                            }
                        }
                    }
                    _ => {
                        let minutes = u64_arg(args, "minutes", 30).clamp(1, 1440) as u32;
                        match self
                            .discord
                            .fetch_messages_recent(target_channel, minutes)
                            .await
                        {
                            Err(e) => format!("Error: {e}"),
                            Ok(msgs) if msgs.is_empty() => {
                                format!("No messages found in the last {minutes} minutes.")
                            }
                            Ok(msgs) => msgs
                                .iter()
                                .map(|m| {
                                    format!("[{}] [{}] {}: {}", m.id, m.ts, m.author, m.content)
                                })
                                .collect::<Vec<_>>()
                                .join("\n"),
                        }
                    }
                })
            }
            "get_current_time" => {
                ToolOutcome::Text(current_time_text(str_arg(args, "timezone"), Utc::now()))
            }
            // Offered only to configurers at the tool-definition layer, but
            // re-checked here as a defence-in-depth measure.
            "configure_bot" => {
                let caller = user_id.parse::<u64>().unwrap_or(0);
                let access = self.access_control.load().await;
                if !access.is_configurer(caller, config::owner_id()) {
                    return ToolOutcome::Text(
                        "Error: permission denied — only users authorized to configure the bot can use this tool."
                            .into(),
                    );
                }
                ToolOutcome::Text(self.handle_configure_bot(args, access).await)
            }
            "read" => ToolOutcome::Text(
                sandbox
                    .read(
                        str_arg(args, "path"),
                        args.get("start_line")
                            .and_then(Value::as_u64)
                            .map(|l| l as u32),
                        args.get("end_line")
                            .and_then(Value::as_u64)
                            .map(|l| l as u32),
                    )
                    .await
                    .unwrap_or_else(|e| format!("Error: {e}")),
            ),
            "write" => ToolOutcome::Text(
                sandbox
                    .write(str_arg(args, "path"), str_arg(args, "content"))
                    .await
                    .unwrap_or_else(|e| format!("Error: {e}")),
            ),
            "edit" => ToolOutcome::Text(
                sandbox
                    .edit(
                        str_arg(args, "path"),
                        str_arg(args, "old_string"),
                        str_arg(args, "new_string"),
                        args.get("replace_all")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    )
                    .await
                    .unwrap_or_else(|e| format!("Error: {e}")),
            ),
            "shell" => ToolOutcome::Text(
                sandbox
                    .shell(
                        str_arg(args, "command"),
                        args.get("working_dir").and_then(Value::as_str),
                        args.get("timeout").and_then(Value::as_u64),
                    )
                    .await
                    .unwrap_or_else(|e| format!("Error: {e}")),
            ),
            _ => ToolOutcome::Text(format!("Unknown tool: {name}")),
        }
    }

    /// Gate a `get_messages` call on the requesting user's own access to the
    /// channel being read, returning the message to show them when refused.
    ///
    /// The buffer holds every channel the bot can see, and both the Discord
    /// fetch paths run with the *bot's* permissions, so without this a user
    /// could read a channel they cannot open themselves. Reading the channel
    /// the conversation is already happening in needs no check — they are
    /// demonstrably in it. Anything else fails closed.
    async fn authorize_channel_read(
        &self,
        guild_id: u64,
        user_id: &str,
        current_channel: u64,
        target_channel: u64,
    ) -> Result<(), String> {
        if target_channel == current_channel {
            return Ok(());
        }
        if guild_id == 0 {
            return Err(
                "Error: messages from a server channel cannot be read from a DM.".to_string(),
            );
        }
        let Ok(user) = user_id.parse::<u64>() else {
            return Err("Error: could not verify your access to that channel.".to_string());
        };
        match self
            .discord
            .can_view_channel(guild_id, user, target_channel)
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(
                "Error: you do not have access to that channel, so I can't read it for you."
                    .to_string(),
            ),
            Err(error) => {
                tracing::warn!(
                    target: "housebot::agent",
                    user_id,
                    target_channel,
                    %error,
                    "channel access check failed — denying the read"
                );
                Err("Error: could not verify your access to that channel.".to_string())
            }
        }
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
    use super::optional_nonzero_u32;
    use serde_json::json;

    #[test]
    fn optional_values_only_clear_when_omitted() {
        assert_eq!(
            optional_nonzero_u32(&json!({}), "max_output_tokens"),
            Ok(None)
        );

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
