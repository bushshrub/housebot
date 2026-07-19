//! Built-in tool dispatch (the large match over tool names).

use super::*;

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
                        u64_arg(args, "max_results", 10) as usize,
                        str_arg(args, "language"),
                    )
                    .await,
            ),
            "deep_research" => {
                let questions: Vec<String> = args
                    .get("questions")
                    .and_then(Value::as_array)
                    .map(|questions| {
                        questions
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                ToolOutcome::Text(
                    self.searxng
                        .deep_research(
                            str_arg(args, "topic"),
                            &questions,
                            u64_arg(args, "max_results_per_query", 5) as usize,
                            str_arg(args, "language"),
                        )
                        .await,
                )
            }
            "fetch_webpage" => ToolOutcome::Text(
                self.web_fetch
                    .fetch_content(
                        str_arg(args, "url"),
                        u64_arg(args, "start_index", 0) as usize,
                        u64_arg(args, "max_length", 8000) as usize,
                    )
                    .await,
            ),
            "download_file" => match self
                .file_downloader
                .download(str_arg(args, "url"), str_arg(args, "filename"))
                .await
            {
                Ok(file) => ToolOutcome::Attachment {
                    text: format!(
                        "Attached `{}` ({} bytes{}) to the Discord response.",
                        file.filename,
                        file.bytes.len(),
                        file.content_type
                            .as_deref()
                            .map(|content_type| format!(", {content_type}"))
                            .unwrap_or_default()
                    ),
                    attachment: AgentAttachment {
                        filename: file.filename,
                        bytes: file.bytes,
                    },
                },
                Err(error) => ToolOutcome::Text(error),
            },
            "common_crawl__search" => ToolOutcome::Text(
                self.common_crawl
                    .search(
                        str_arg(args, "pattern"),
                        str_arg(args, "crawl"),
                        args.get("match_type")
                            .and_then(Value::as_str)
                            .unwrap_or("exact"),
                        u64_arg(args, "max_results", 10) as usize,
                    )
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
                    let query_lower = query.to_lowercase();
                    let matching: Vec<&str> = content
                        .lines()
                        .filter(|line| line.to_lowercase().contains(&query_lower))
                        .collect();
                    if matching.is_empty() {
                        ToolOutcome::Text(format!("No memory entries matching '{query}'."))
                    } else {
                        ToolOutcome::Text(matching.join("\n"))
                    }
                }
            }
            "github_api" => ToolOutcome::Text(
                tools::github_api::handle_github_api(&self.reporter, str_arg(args, "action"), args)
                    .await,
            ),
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
                let interactive = args
                    .get("interactive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);

                let dispatch_mode = if requester_user_id == owner_id {
                    if interactive {
                        DispatchMode::Interactive
                    } else {
                        DispatchMode::Immediate
                    }
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
                        "opencode/deepseek-v4-flash-free",
                    );
                    let effort = config::env_or("DEVELOPMENT_DEFAULT_EFFORT", "medium");
                    PartialAgentSelection {
                        agent: CodingAgent::from_str(&agent_str).ok(),
                        model: Some(model),
                        effort: Some(effort),
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
                    FeatureDevelopmentOutcome::OwnerDispatchReady { job_id } => {
                        Some(AgentControlAction::OwnerDispatchReady { job_id: *job_id })
                    }
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
            "summarize_url" => ToolOutcome::Text(
                tools::summarize_url::fetch_and_summarize(
                    &*self.client,
                    &self.model,
                    str_arg(args, "url"),
                )
                .await,
            ),
            "translate" => ToolOutcome::Text(
                tools::translate::translate_text(
                    &*self.client,
                    &self.model,
                    str_arg(args, "text"),
                    str_arg(args, "target_language"),
                )
                .await,
            ),
            "search_location" => ToolOutcome::Text(
                self.osm_client
                    .search_location(str_arg(args, "query"), u64_arg(args, "limit", 3) as usize)
                    .await,
            ),
            "lookup_coordinates" => ToolOutcome::Text(
                self.osm_client
                    .lookup_coordinates(
                        args.get("latitude").and_then(Value::as_f64).unwrap_or(0.0),
                        args.get("longitude").and_then(Value::as_f64).unwrap_or(0.0),
                    )
                    .await,
            ),
            "create_skill" => ToolOutcome::Text(
                tools::create_skill::dispatch_create_skill(&self.skills, user_id, args).await,
            ),
            "get_bot_features" => ToolOutcome::Text(tools::features::features_text().to_string()),
            "get_token_metrics" => ToolOutcome::Text(
                tools::token_metrics::get_token_metrics(
                    &self.token_monitor,
                    args.get("user_id").and_then(Value::as_str),
                    args.get("period").and_then(Value::as_str),
                    args.get("metric").and_then(Value::as_str),
                )
                .await,
            ),
            "search_messages" => {
                let query = str_arg(args, "query");
                let max_results = u64_arg(args, "max_results", 10).clamp(1, 20) as usize;
                let target_channel = args
                    .get("channel_id")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(channel_id);
                ToolOutcome::Text(
                    match self
                        .channel_log
                        .search(target_channel, query, max_results)
                        .await
                    {
                        Err(e) => format!("Error: {e}"),
                        Ok(msgs) if msgs.is_empty() => "No matching messages found.".to_string(),
                        Ok(msgs) => msgs
                            .iter()
                            .map(|m| {
                                let author = m.nick.as_deref().unwrap_or(&m.username);
                                format!("[{}] {}: {}", m.ts, author, m.content)
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    },
                )
            }
            "get_recent_messages" => {
                let minutes = u64_arg(args, "minutes", 30).clamp(1, 1440) as u32;
                let target_channel = args
                    .get("channel_id")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(channel_id);
                ToolOutcome::Text(
                    match self.channel_log.get_recent(target_channel, minutes).await {
                        Err(e) => format!("Error: {e}"),
                        Ok(msgs) if msgs.is_empty() => {
                            format!("No messages found in the last {minutes} minutes.")
                        }
                        Ok(msgs) => msgs
                            .iter()
                            .map(|m| {
                                let author = m.nick.as_deref().unwrap_or(&m.username);
                                format!("[{}] {}: {}", m.ts, author, m.content)
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    },
                )
            }
            "find_discord_users" => {
                let query = str_arg(args, "query");
                let max_results = u64_arg(args, "max_results", 10).clamp(1, 20) as usize;
                let target_channel = args
                    .get("channel_id")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(channel_id);
                ToolOutcome::Text(
                    match self
                        .channel_log
                        .find_authors(target_channel, query, max_results)
                        .await
                    {
                        Err(error) => format!("Error: {error}"),
                        Ok(authors) if authors.is_empty() => {
                            "No matching Discord users found in this channel's history.".to_string()
                        }
                        Ok(authors) => authors
                            .iter()
                            .map(|author| {
                                let nick = author.nick.as_deref().unwrap_or("(none)");
                                format!(
                                    "Username: {} | Nickname: {} | ID: {}",
                                    author.username, nick, author.user_id
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    },
                )
            }
            "get_discord_user" => {
                let uid: u64 = str_arg(args, "user_id").parse().unwrap_or(0);
                ToolOutcome::Text(if uid == 0 {
                    "Error: invalid user_id.".to_string()
                } else {
                    match self.discord.fetch_user(uid).await {
                        Ok(u) => {
                            let avatar = u.avatar_url.as_deref().unwrap_or("(none)");
                            format!(
                                "Username: {}\nDisplay name: {}\nID: {}\nBot: {}\nAccount created: {}\nAvatar URL: {}",
                                u.username, u.display_name, u.id, u.bot, u.created_at, avatar
                            )
                        }
                        Err(e) => format!("Error: {e}"),
                    }
                })
            }
            "run_lua" => {
                let script = lua_engine::strip_code_fence(str_arg(args, "script")).to_string();
                let host = Arc::new(AgentScriptHost {
                    searxng: Arc::clone(&self.searxng),
                    mcp_servers: Arc::clone(&self.mcp_servers),
                });
                let output = lua_engine::run_script(
                    script,
                    host,
                    lua_engine::LuaLimits::from_env(),
                    |s: &str| s.to_string(),
                )
                .await;
                if let Some(image) = output.image {
                    let text = if output.text.is_empty() {
                        format!(
                            "Graph rendered as PNG ({} bytes) and attached to the Discord response.",
                            image.len()
                        )
                    } else {
                        format!(
                            "{}\n\nA graph PNG image ({} bytes) was also generated and \
                             automatically attached to the Discord response.",
                            output.text,
                            image.len()
                        )
                    };
                    ToolOutcome::Attachment {
                        text,
                        attachment: AgentAttachment {
                            filename: "graph.png".to_string(),
                            bytes: image,
                        },
                    }
                } else {
                    ToolOutcome::Text(output.text)
                }
            }
            "get_lua_docs" => ToolOutcome::Text(LUA_DOCS.to_string()),
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
            // ── Sandbox tools (owner-only; enforced at the tool-definition
            //    layer, but re-checked here as a defence-in-depth measure) ──
            name if name.starts_with("sandbox_") => {
                let is_owner = user_id.parse::<u64>().unwrap_or(0) == config::owner_id();
                if !is_owner {
                    return ToolOutcome::Text(
                        "Error: permission denied — sandbox tools are owner-only.".into(),
                    );
                }
                match name {
                    "sandbox_clone_repository" => ToolOutcome::Text(
                        sandbox
                            .clone_repository(
                                str_arg(args, "url"),
                                args.get("branch").and_then(Value::as_str),
                            )
                            .await
                            .unwrap_or_else(|e| format!("Error: {e}")),
                    ),
                    "sandbox_list_files" => ToolOutcome::Text(
                        sandbox
                            .list_files(
                                str_arg(args, "path"),
                                args.get("max_depth")
                                    .and_then(Value::as_u64)
                                    .map(|d| d as u32),
                            )
                            .await
                            .unwrap_or_else(|e| format!("Error: {e}")),
                    ),
                    "sandbox_search_code" => ToolOutcome::Text(
                        sandbox
                            .search_code(
                                str_arg(args, "query"),
                                args.get("path").and_then(Value::as_str),
                                args.get("glob").and_then(Value::as_str),
                            )
                            .await
                            .unwrap_or_else(|e| format!("Error: {e}")),
                    ),
                    "sandbox_read_file" => ToolOutcome::Text(
                        sandbox
                            .read_file(
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
                    "sandbox_run" => ToolOutcome::Text(
                        sandbox
                            .run(
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
            _ if name.contains("__") => {
                let (prefix, tool_name) = name.split_once("__").unwrap();
                for server in self.mcp_servers.iter() {
                    if server.prefix == prefix {
                        return match server.call_tool(tool_name, args.clone()).await {
                            Ok(text) => ToolOutcome::Text(text),
                            Err(e) => ToolOutcome::Text(format!("Error: {e}")),
                        };
                    }
                }
                ToolOutcome::Text(format!("Unknown tool: {name}"))
            }
            _ => ToolOutcome::Text(format!("Unknown tool: {name}")),
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
