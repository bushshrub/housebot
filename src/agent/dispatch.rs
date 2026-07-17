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
            "run_skill" => {
                let skill_name = str_arg(args, "name");
                let input = str_arg(args, "input");
                match self.skills.get(skill_name).await {
                    None => ToolOutcome::Text(format!("Error: Skill '{skill_name}' not found.")),
                    Some(skill) => {
                        let msgs = vec![
                            json!({"role": "system", "content": skill.prompt}),
                            json!({"role": "user", "content": input}),
                        ];
                        let completion = self
                            .client
                            .chat_once(&self.model, &msgs, 4096)
                            .await
                            .unwrap_or_default();
                        ToolOutcome::Text(completion.content.unwrap_or_default())
                    }
                }
            }
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
                // Only `.text` is surfaced here — no redaction needed for the
                // graph image path since it's never attached from this tool.
                ToolOutcome::Text(
                    lua_engine::run_script(
                        script,
                        host,
                        lua_engine::LuaLimits::from_env(),
                        |s: &str| s.to_string(),
                    )
                    .await
                    .text,
                )
            }
            "get_lua_docs" => ToolOutcome::Text(LUA_DOCS.to_string()),
            // ── Sandbox tools (owner-only; enforced at the tool-definition
            //    layer, but re-checked here as a defence-in-depth measure) ──
            "sandbox_clone_repository" => {
                let is_owner = user_id.parse::<u64>().unwrap_or(0) == config::owner_id();
                if !is_owner {
                    return ToolOutcome::Text(
                        "Error: permission denied — sandbox tools are owner-only.".into(),
                    );
                }
                ToolOutcome::Text(
                    sandbox
                        .clone_repository(
                            str_arg(args, "url"),
                            args.get("branch").and_then(Value::as_str),
                        )
                        .await
                        .unwrap_or_else(|e| format!("Error: {e}")),
                )
            }
            "sandbox_list_files" => {
                let is_owner = user_id.parse::<u64>().unwrap_or(0) == config::owner_id();
                if !is_owner {
                    return ToolOutcome::Text(
                        "Error: permission denied — sandbox tools are owner-only.".into(),
                    );
                }
                ToolOutcome::Text(
                    sandbox
                        .list_files(
                            str_arg(args, "path"),
                            args.get("max_depth")
                                .and_then(Value::as_u64)
                                .map(|d| d as u32),
                        )
                        .await
                        .unwrap_or_else(|e| format!("Error: {e}")),
                )
            }
            "sandbox_search_code" => {
                let is_owner = user_id.parse::<u64>().unwrap_or(0) == config::owner_id();
                if !is_owner {
                    return ToolOutcome::Text(
                        "Error: permission denied — sandbox tools are owner-only.".into(),
                    );
                }
                ToolOutcome::Text(
                    sandbox
                        .search_code(
                            str_arg(args, "query"),
                            args.get("path").and_then(Value::as_str),
                            args.get("glob").and_then(Value::as_str),
                        )
                        .await
                        .unwrap_or_else(|e| format!("Error: {e}")),
                )
            }
            "sandbox_read_file" => {
                let is_owner = user_id.parse::<u64>().unwrap_or(0) == config::owner_id();
                if !is_owner {
                    return ToolOutcome::Text(
                        "Error: permission denied — sandbox tools are owner-only.".into(),
                    );
                }
                ToolOutcome::Text(
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
                )
            }
            "sandbox_run" => {
                let is_owner = user_id.parse::<u64>().unwrap_or(0) == config::owner_id();
                if !is_owner {
                    return ToolOutcome::Text(
                        "Error: permission denied — sandbox tools are owner-only.".into(),
                    );
                }
                ToolOutcome::Text(
                    sandbox
                        .run(
                            str_arg(args, "command"),
                            args.get("working_dir").and_then(Value::as_str),
                            args.get("timeout").and_then(Value::as_u64),
                        )
                        .await
                        .unwrap_or_else(|e| format!("Error: {e}")),
                )
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
}
