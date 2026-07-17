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

        let tools = self.build_tools(deep_memory_enabled).await;
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
            let completion = match self
                .client
                .chat_stream(
                    &self.model,
                    &messages,
                    &tools,
                    None,
                    thinking,
                    Some(&text_sink),
                )
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("LLM error: {e}");
                    break "Sorry, something went wrong contacting the model.".to_string();
                }
            };
            let context_tokens =
                completion.usage.prompt_tokens + completion.usage.completion_tokens;
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
                    .dispatch_tool(&tc.name, &args, user_id, username, channel_id, guild_id)
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

    pub(crate) async fn build_tools(&self, deep_memory_enabled: bool) -> Vec<Value> {
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
            tools::feature_request::definition(),
            tools::edit_feature_request::definition(),
            tools::feature_development::definition(),
            tools::remind::definition(),
            tools::summarize_url::definition(),
            tools::token_metrics::definition(),
            tools::translate::definition(),
            tools::features::definition(),
            search_messages_tool(),
            get_recent_messages_tool(),
            find_discord_users_tool(),
            get_discord_user_tool(),
            run_lua_tool(),
            get_lua_docs_tool(),
        ];
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

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn dispatch_tool(
        &self,
        name: &str,
        args: &Value,
        user_id: &str,
        username: &str,
        channel_id: u64,
        guild_id: Option<u64>,
    ) -> ToolOutcome {
        let started = std::time::Instant::now();
        let requester_id = user_id.parse().unwrap_or(0);
        let outcome = if let Some(guild_id) = guild_id {
            match self
                .tool_permissions
                .is_banned(guild_id, requester_id, name)
                .await
            {
                Ok(true) => ToolOutcome::Text(format!(
                    "Error: permission denied — you are restricted from using `{name}` in this server."
                )),
                Ok(false) => {
                    self.dispatch_tool_inner(name, args, user_id, username, channel_id, guild_id)
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
            self.dispatch_tool_inner(name, args, user_id, username, channel_id, 0)
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
