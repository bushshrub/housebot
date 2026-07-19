//! The main agent loop and tool dispatch entry points.

use super::*;

impl Agent {
    // ── main loop ────────────────────────────────────────────────────────────

    /// Run one user turn to completion, returning the final assistant text.
    pub async fn run(&self, request: AgentRequest<'_>, hooks: &dyn AgentHooks) -> AgentResult {
        let AgentRequest {
            user_id,
            username,
            text,
            media,
            personality,
            thinking,
            channel_id,
            deep_memory_enabled,
            display_name,
            nickname,
            avatar_url,
            profile_tags,
            quick_actions,
            guild_id,
            proactive,
            record_profile_usage,
            max_output_tokens,
        } = request;
        let run_started = std::time::Instant::now();
        tracing::info!(
            target: "housebot::agent",
            user_id,
            username,
            thinking = %thinking,
            text_chars = text.chars().count(),
            media = media.len(),
            "Agent run started"
        );
        let mut user_memory = self.memory.load(user_id).await;
        let mut past = self.history.load(user_id).await;
        let mut session_notice = None;
        let new_user_message = build_user_message(text, media);
        let mut history_user_message = new_user_message.clone();
        history_user_message["discord_context"] = json!({
            "guild_id": guild_id,
            "channel_id": channel_id,
            "timestamp": Utc::now().to_rfc3339(),
            "username": username,
            "display_name": display_name,
            "avatar_url": avatar_url,
        });

        let previous_usage = self.last_context_tokens(user_id).await as f64
            / self.context_window_tokens.max(1) as f64;
        if !past.is_empty() && previous_usage >= 0.9 {
            tracing::info!("Context at 90% for {user_id} — auto-compacting session");
            self.compact_session_with_hooks(user_id, deep_memory_enabled, hooks)
                .await;
            past.clear();
            user_memory = self.memory.load(user_id).await;
            session_notice = Some(
                "⚠️ The context window reached 90%, so I compacted the conversation and started a new session. Use /session to check your current context usage."
                    .into(),
            );
        }
        let conversation_id = self
            .current_conversation_id(user_id, display_name, channel_id)
            .await;

        let all_skills = self.skills.load_all().await;
        let now = Local::now().format("%Y-%m-%d %H:%M").to_string();
        let system = json!({
            "role": "system",
            "content": build_system_prompt_with_profile(
                username,
                user_id,
                display_name,
                nickname,
                avatar_url,
                &user_memory,
                &all_skills,
                personality,
                deep_memory_enabled,
                profile_tags,
                quick_actions,
                &now,
            ),
        });
        let mut messages: Vec<Value> = Vec::with_capacity(past.len() + 2);
        messages.push(system);
        messages.extend(past);
        messages.push(new_user_message.clone());

        let is_owner = user_id.parse::<u64>().unwrap_or(0) == config::owner_id();
        let is_configurer = self
            .access_control
            .load()
            .await
            .is_configurer(user_id.parse::<u64>().unwrap_or(0), config::owner_id());
        let tools = self
            .build_tools(deep_memory_enabled, is_owner, is_configurer)
            .await;
        let sandbox = LazySandbox::new(self.sandbox_client.clone());
        let mut turn_messages: Vec<Value> = Vec::new();
        let mut tools_called = Vec::new();
        let mut attachments = Vec::new();

        let mut control_action: Option<AgentControlAction> = None;

        // Bound the tool loop so a model that keeps requesting tools cannot
        // spin forever (each iteration is a full LLM round trip).
        const MAX_TOOL_ROUNDS: usize = 16;
        let mut rounds = 0;
        let final_text = loop {
            rounds += 1;
            if rounds > MAX_TOOL_ROUNDS {
                tracing::warn!(target: "housebot::agent", user_id, "Tool loop exceeded {MAX_TOOL_ROUNDS} rounds — stopping");
                break "I had to stop because this request required too many tool calls in a row. Please try a more specific request.".to_string();
            }
            let text_sink = TextStreamAdapter(hooks);
            let completion_result = self
                .client
                .chat_stream(
                    &self.model,
                    &messages,
                    &tools,
                    None,
                    thinking,
                    max_output_tokens,
                    Some(&text_sink),
                )
                .await;
            hooks.on_text_stream_end().await;
            let completion = match completion_result {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("LLM error: {e}");
                    break "Sorry, something went wrong contacting the model.".to_string();
                }
            };
            let context_tokens = completion.usage.prompt_tokens;
            self.record_usage(user_id, &conversation_id, completion.usage)
                .await;
            let usage = context_tokens as f64 / self.context_window_tokens.max(1) as f64;
            if usage >= 0.8 {
                session_notice = Some(if usage >= 0.9 {
                    "⚠️ The context window reached 90% based on the model's reported usage. It will be compacted automatically before the next message. Use /session to check your current context usage.".into()
                } else {
                    format!(
                        "⚠️ The context window is {:.0}% full based on the model's reported usage. It will compact automatically at 90%. Use /session to check your current context usage.",
                        usage * 100.0
                    )
                });
            }

            let mut assistant = json!({ "role": "assistant", "content": completion.content });
            if !completion.tool_calls.is_empty() {
                assistant["tool_calls"] = Value::Array(
                    completion
                        .tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {"name": tc.name, "arguments": tc.arguments},
                            })
                        })
                        .collect(),
                );
            }
            messages.push(assistant.clone());
            turn_messages.push(assistant);

            let is_tool_turn = completion.finish_reason.as_deref() == Some("tool_calls")
                && !completion.tool_calls.is_empty();
            if !is_tool_turn {
                break completion.content.unwrap_or_default();
            }

            // Search rate limits are not recoverable within this run. After the
            // first limited response the loop stops — but only once every tool
            // call in this batch has received a tool message, so the persisted
            // history never contains a tool call without its result.
            let mut rate_limited = false;
            for tc in &completion.tool_calls {
                let args: Value = serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
                tools_called.push(tc.name.clone());
                hooks.on_tool_called(&tc.name, &args).await;
                let outcome = self
                    .dispatch_tool(
                        &tc.name, &args, user_id, username, channel_id, guild_id, &sandbox,
                    )
                    .await;
                let content = match outcome {
                    ToolOutcome::Text(ref t) => t.clone(),
                    ToolOutcome::Attachment { text, attachment } => {
                        attachments.push(attachment);
                        text
                    }
                    ToolOutcome::DevelopmentAction {
                        ref text,
                        ref action,
                    } => {
                        control_action = Some(action.clone());
                        text.clone()
                    }
                };
                let tool_msg = json!({
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "content": content,
                });
                messages.push(tool_msg.clone());
                turn_messages.push(tool_msg);

                if matches!(tc.name.as_str(), "web_search" | "deep_research" | "run_lua")
                    && search_rate_limited(&content)
                {
                    rate_limited = true;
                }
            }
            if rate_limited {
                break "Web search is temporarily rate-limited. Please try again in a few minutes."
                    .to_string();
            }
        };

        sandbox.close().await;

        if let Err(error) = self
            .token_monitor
            .record_turn(&conversation_id, &history_user_message, &turn_messages)
            .await
        {
            tracing::error!(%error, %user_id, %conversation_id, "failed to archive conversation turn");
        }

        if let Err(e) = self
            .history
            .append_turn(user_id, history_user_message, turn_messages)
            .await
        {
            tracing::error!("Failed to save history for {user_id}: {e}");
        }

        // Record only direct-turn tool usage in the user's profile. Proactive
        // replies must not learn profile tags from unsolicited messages.
        if record_profile_usage && !proactive && !tools_called.is_empty() {
            let mut profile = self.profile_store.load(user_id).await;
            for tool_name in &tools_called {
                profile.record_tool_use(tool_name);
            }
            let _ = self.profile_store.save(user_id, &profile).await;
        }

        tracing::info!(
            target: "housebot::agent",
            user_id,
            tools_called = tools_called.len(),
            response_chars = final_text.chars().count(),
            elapsed_ms = run_started.elapsed().as_millis() as u64,
            "Agent run finished"
        );
        AgentResult {
            text: if final_text.is_empty() {
                "(no response)".to_string()
            } else {
                final_text
            },
            session_notice,
            tools_called,
            attachments,
            control_action,
        }
    }

    pub(crate) async fn build_tools(
        &self,
        deep_memory_enabled: bool,
        sandbox_allowed: bool,
        configurer: bool,
    ) -> Vec<Value> {
        let mut tools = Vec::new();
        for server in self.mcp_servers.iter() {
            for tool in server.list_tools().await {
                tools.push(to_openai_tool(
                    &format!("{}__{}", server.prefix, tool.name),
                    &tool.description,
                    tool.input_schema,
                ));
            }
        }
        let mut defs: Vec<Value> = vec![
            tools::searxng::definition(),
            tools::searxng::deep_research_definition(),
            tools::web_fetch::definition(),
            tools::file_download::definition(),
            tools::common_crawl::definition(),
            run_skill_tool(),
            create_skill_tool(),
            tools::feature_request::definition(),
            tools::edit_feature_request::definition(),
            tools::feature_development::definition(),
            tools::github_api::definition(),
            tools::remind::definition(),
            tools::summarize_url::definition(),
            tools::token_metrics::definition(),
            tools::translate::definition(),
            tools::features::definition(),
            get_messages_tool(),
            find_discord_users_tool(),
            get_discord_user_tool(),
            run_lua_tool(),
            get_lua_docs_tool(),
        ];
        // Include sandbox tools only for the owner.
        if sandbox_allowed {
            defs.extend(tools::sandbox::all_definitions());
        }
        // Configuration control is only offered to authorized configurers
        // (re-checked at dispatch as a defence-in-depth measure).
        if configurer {
            defs.push(configure_bot_tool());
        }
        // Conditionally include memory tools based on user's privacy setting.
        if deep_memory_enabled {
            defs.push(crate::memory::update_memory_tool());
            defs.push(crate::memory::search_memory_tool());
        }
        for def in defs {
            let (name, desc, params) = flatten_tool(&def);
            tools.push(to_openai_tool(&name, &desc, params));
        }
        tools
    }

    /// Build OpenAI tool definitions for only the tools in `enabled_tools`,
    /// matching against built-in definitions and MCP server tools.
    pub(crate) async fn build_skill_tools(&self, enabled_tools: &[String]) -> Vec<Value> {
        // Collect all tool definitions from the same sources as build_tools,
        // then filter to only those in the enabled list.
        let mut all = Vec::new();
        for server in self.mcp_servers.iter() {
            for tool in server.list_tools().await {
                all.push((
                    format!("{}__{}", server.prefix, tool.name),
                    tool.description,
                    tool.input_schema,
                ));
            }
        }
        let builtin_defs: Vec<Value> = vec![
            tools::searxng::definition(),
            tools::searxng::deep_research_definition(),
            tools::web_fetch::definition(),
            tools::file_download::definition(),
            tools::common_crawl::definition(),
            run_skill_tool(),
            create_skill_tool(),
            tools::feature_request::definition(),
            tools::edit_feature_request::definition(),
            tools::feature_development::definition(),
            tools::github_api::definition(),
            tools::remind::definition(),
            tools::summarize_url::definition(),
            tools::token_metrics::definition(),
            tools::translate::definition(),
            tools::features::definition(),
            get_messages_tool(),
            find_discord_users_tool(),
            get_discord_user_tool(),
            run_lua_tool(),
            get_lua_docs_tool(),
        ];
        for def in builtin_defs {
            let (name, desc, params) = flatten_tool(&def);
            all.push((name, desc, params));
        }

        let mut tools = Vec::new();
        for enabled in enabled_tools {
            if let Some((name, desc, params)) = all.iter().find(|(n, _, _)| n == enabled) {
                tools.push(to_openai_tool(name, desc, params.clone()));
            }
        }
        tools
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn dispatch_tool(
        &self,
        name: &str,
        args: &Value,
        user_id: &str,
        username: &str,
        channel_id: u64,
        guild_id: Option<u64>,
        sandbox: &LazySandbox,
    ) -> ToolOutcome {
        let started = std::time::Instant::now();
        let requester_id = user_id.parse().unwrap_or(0);
        let outcome = if name == "run_skill" {
            // Guild permission check before skill execution.
            if let Some(gid) = guild_id {
                match self
                    .tool_permissions
                    .is_banned(gid, requester_id, "run_skill")
                    .await
                {
                    Ok(true) => {
                        return ToolOutcome::Text(
                            "Error: permission denied — you are restricted from using `run_skill` in this server."
                                .into(),
                        )
                    }
                    Err(error) => {
                        tracing::error!(%error, %gid, "tool permission check failed for run_skill");
                        return ToolOutcome::Text(
                            "Error: tool permissions are temporarily unavailable; the tool call was blocked for safety."
                                .into(),
                        );
                    }
                    Ok(false) => {}
                }
            }
            let skill_name = str_arg(args, "name");
            let input = str_arg(args, "input");
            match self.skills.get(skill_name).await {
                None => ToolOutcome::Text(format!("Error: Skill '{skill_name}' not found.")),
                Some(skill) => {
                    let instructions = skill.effective_instructions();
                    let system = build_skill_system_prompt(&skill, instructions);
                    let tools = self.build_skill_tools(&skill.enabled_tools).await;

                    let mut messages: Vec<Value> = vec![
                        json!({"role": "system", "content": system}),
                        json!({"role": "user", "content": input}),
                    ];

                    const MAX_SKILL_ROUNDS: usize = 8;
                    let mut rounds = 0usize;
                    let mut skill_attachments: Vec<AgentAttachment> = Vec::new();
                    let mut skill_control_action: Option<AgentControlAction> = None;
                    let final_text = loop {
                        rounds += 1;
                        if rounds > MAX_SKILL_ROUNDS {
                            break "Skill execution exceeded maximum tool rounds and was stopped."
                                .to_string();
                        }

                        let completion = match self
                            .client
                            .chat_stream(
                                &self.model,
                                &messages,
                                &tools,
                                None,
                                ThinkingMode::default(),
                                None,
                                None,
                            )
                            .await
                        {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::error!(%e, "run_skill LLM error");
                                break "Error: LLM call failed during skill execution.".to_string();
                            }
                        };

                        let mut assistant_msg = json!({
                            "role": "assistant",
                            "content": completion.content,
                        });
                        if !completion.tool_calls.is_empty() {
                            assistant_msg["tool_calls"] = Value::Array(
                                completion.tool_calls.iter().map(|tc| {
                                    json!({
                                        "id": tc.id,
                                        "type": "function",
                                        "function": {"name": tc.name, "arguments": tc.arguments},
                                    })
                                }).collect(),
                            );
                        }
                        messages.push(assistant_msg);

                        let is_tool_turn = completion.finish_reason.as_deref()
                            == Some("tool_calls")
                            && !completion.tool_calls.is_empty();
                        if !is_tool_turn {
                            break completion.content.unwrap_or_default();
                        }

                        let gid = guild_id.unwrap_or(0);
                        for tc in &completion.tool_calls {
                            let tc_args: Value =
                                serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
                            if tc.name == "run_skill" {
                                messages.push(json!({
                                    "role": "tool",
                                    "tool_call_id": tc.id,
                                    "content": "Error: run_skill cannot be called recursively.",
                                }));
                                continue;
                            }
                            // Permission check for each nested tool.
                            if guild_id.is_some() {
                                match self
                                    .tool_permissions
                                    .is_banned(gid, requester_id, &tc.name)
                                    .await
                                {
                                    Ok(true) => {
                                        messages.push(json!({
                                            "role": "tool",
                                            "tool_call_id": tc.id,
                                            "content": format!("Error: permission denied — you are restricted from using `{}` in this server.", tc.name),
                                        }));
                                        continue;
                                    }
                                    Err(error) => {
                                        tracing::error!(%error, %gid, "tool permission check failed during skill execution");
                                        messages.push(json!({
                                            "role": "tool",
                                            "tool_call_id": tc.id,
                                            "content": "Error: tool permissions are temporarily unavailable.",
                                        }));
                                        continue;
                                    }
                                    Ok(false) => {}
                                }
                            }
                            let outcome = self
                                .dispatch_tool_inner(
                                    &tc.name, &tc_args, user_id, username, channel_id, gid, sandbox,
                                )
                                .await;
                            let content = match &outcome {
                                ToolOutcome::Text(t) => t.clone(),
                                ToolOutcome::Attachment { text, attachment } => {
                                    skill_attachments.push(attachment.clone());
                                    text.clone()
                                }
                                ToolOutcome::DevelopmentAction { text, action } => {
                                    skill_control_action = Some(action.clone());
                                    text.clone()
                                }
                            };
                            messages.push(json!({
                                "role": "tool",
                                "tool_call_id": tc.id,
                                "content": content,
                            }));
                        }
                    };

                    if let Some(action) = skill_control_action {
                        ToolOutcome::DevelopmentAction {
                            text: final_text,
                            action,
                        }
                    } else if let Some(attachment) = skill_attachments.into_iter().next() {
                        ToolOutcome::Attachment {
                            text: final_text,
                            attachment,
                        }
                    } else {
                        ToolOutcome::Text(final_text)
                    }
                }
            }
        } else if let Some(guild_id) = guild_id {
            match self
                .tool_permissions
                .is_banned(guild_id, requester_id, name)
                .await
            {
                Ok(true) => ToolOutcome::Text(format!(
                    "Error: permission denied — you are restricted from using `{name}` in this server."
                )),
                Ok(false) => {
                    self.dispatch_tool_inner(name, args, user_id, username, channel_id, guild_id, sandbox)
                        .await
                }
                Err(error) => {
                    tracing::error!(%error, %guild_id, "tool permission check failed");
                    ToolOutcome::Text(
                        "Error: tool permissions are temporarily unavailable; the tool call was blocked for safety."
                            .into(),
                    )
                }
            }
        } else {
            self.dispatch_tool_inner(name, args, user_id, username, channel_id, 0, sandbox)
                .await
        };
        let content = match &outcome {
            ToolOutcome::Text(t) => t.as_str(),
            ToolOutcome::Attachment { text, .. } => text.as_str(),
            ToolOutcome::DevelopmentAction { text, .. } => text.as_str(),
        };
        tracing::info!(
            target: "housebot::agent",
            user_id,
            tool = name,
            result_chars = content.chars().count(),
            is_error = content.starts_with("Error:"),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Tool call finished"
        );
        outcome
    }
}
