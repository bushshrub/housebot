//! Token-leaderboard command and the core message-handling flow.

use super::*;

pub(crate) enum ResponseMode {
    Full { proactive: bool },
    EmojiOrFull,
}

impl HouseBot {
    pub(crate) async fn handle_message(
        &self,
        ctx: &Context,
        msg: &Message,
        bot_id: UserId,
        session_expired: bool,
        followup_timeout: Duration,
        response_mode: ResponseMode,
    ) {
        let proactive = matches!(response_mode, ResponseMode::Full { proactive: true });
        let emoji_only_allowed = matches!(response_mode, ResponseMode::EmojiOrFull);
        let mut text = msg.content.clone();
        for token in [format!("<@{bot_id}>"), format!("<@!{bot_id}>")] {
            text = text.replace(&token, "");
        }
        let text = text.trim().to_string();
        let attachment_text = message_attachment_context(msg);
        let text = match attachment_text {
            Some(attachments) if text.is_empty() => attachments,
            Some(attachments) => format!("{text}\n\n{attachments}"),
            None => text,
        };
        if text.is_empty() && !message_has_attachments(msg) {
            return;
        }

        if self
            .chat_rate_limiter
            .check(&msg.author.id.get().to_string())
        {
            tracing::warn!(
                target: "housebot::rate_limit",
                user_id = msg.author.id.get(),
                "Chat rate limit exceeded"
            );
            self.respond(ctx, msg, "⏱️ You're sending messages too quickly. Please slow down and try again in a moment.").await;
            return;
        }

        let user_config = self.user_cfg.load(msg.author.id.get()).await;

        // Check for a full bot ban (`housebot` tool name) in guild channels.
        if let Some(guild_id) = msg.guild_id.map(|g| g.get()) {
            match self
                .agent
                .tool_permissions()
                .is_banned(guild_id, msg.author.id.get(), "housebot")
                .await
            {
                Ok(true) => {
                    tracing::info!(
                        target: "housebot::commands",
                        user_id = msg.author.id.get(),
                        guild_id,
                        "Blocked message from user banned from bot",
                    );
                    self.respond(
                        ctx,
                        msg,
                        "⛔ You are banned from using this bot in this server.",
                    )
                    .await;
                    return;
                }
                Err(error) => {
                    tracing::error!(%error, %guild_id, "housebot ban check failed");
                }
                _ => {}
            }
        }

        let referenced_text = {
            if let Some(referenced) = msg.referenced_message.as_deref() {
                referenced_message_context(referenced)
            } else if let Some(msg_ref) = msg.message_reference.as_ref() {
                if let Some(msg_id) = msg_ref.message_id {
                    match msg_ref.channel_id.message(&ctx.http, msg_id).await {
                        Ok(fetched) => referenced_message_context(&fetched),
                        Err(error) => {
                            tracing::debug!(
                                target: "housebot::message_flow",
                                channel_id = msg_ref.channel_id.get(),
                                message_id = msg_id.get(),
                                %error,
                                "Failed to fetch referenced message"
                            );
                            None
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            }
        };
        let text = match referenced_text {
            Some(referenced) if text.is_empty() => referenced,
            Some(referenced) => format!("{text}\n\n{referenced}"),
            None => text,
        };
        if emoji_only_allowed && !message_has_attachments(msg) {
            if let Some(emoji) = self.agent.select_emoji(&text).await {
                let reaction = serenity::all::ReactionType::Unicode(emoji);
                match msg.react(&ctx.http, reaction).await {
                    Ok(_) => return,
                    Err(error) => tracing::warn!(
                        target: "housebot::emoji",
                        message_id = msg.id.get(),
                        %error,
                        "Failed to send emoji-only response"
                    ),
                }
            }
        }
        if text.is_empty() && !message_has_attachments(msg) {
            return;
        }

        if session_expired {
            self.agent
                .compact_session(
                    &msg.author.id.get().to_string(),
                    user_config.deep_memory_enabled,
                )
                .await;
        }

        let mut media = extract_media(msg).await;
        if let Some(referenced) = msg.referenced_message.as_deref() {
            media.extend(extract_media(referenced).await);
        }
        media.extend(extract_gif_from_text(&msg.content).await);
        if let Some(referenced) = msg.referenced_message.as_deref() {
            media.extend(extract_gif_from_text(&referenced.content).await);
        }

        // Load per-user settings (personality, thinking effort, and privacy).
        let personality = user_config.personality.clone();
        let thinking = user_config.thinking_mode;
        let max_output_tokens = self
            .access
            .load()
            .await
            .policy(msg.author.id.get())
            .max_output_tokens;

        // Refresh user profile from Discord and persist learned data.
        let mut profile = self.profile_store.load(msg.author.id.get()).await;
        let guild_id = msg.guild_id.map(|g| g.get()).unwrap_or(0);
        if profile.username.is_empty() || profile.guild_id != guild_id {
            // First time seeing this user in this guild — fetch profile from Discord.
            if let Ok(user_info) = self.discord.fetch_user(msg.author.id.get()).await {
                profile.username = user_info.username;
                profile.display_name = user_info.display_name;
                profile.avatar_url = user_info.avatar_url.unwrap_or_default();
                profile.guild_id = guild_id;
                profile.nickname.clear();
                if let Some(guild) = msg.guild(&ctx.cache) {
                    if let Some(member) = guild.members.get(&msg.author.id) {
                        if let Some(nick) = &member.nick {
                            profile.nickname = nick.clone();
                        }
                    }
                }
                let _ = self.profile_store.save(msg.author.id.get(), &profile).await;
            }
        } else {
            // Update display name and nickname if they've changed.
            if let Ok(user_info) = self.discord.fetch_user(msg.author.id.get()).await {
                if profile.display_name != user_info.display_name {
                    profile.display_name = user_info.display_name;
                }
                let avatar = user_info.avatar_url.clone().unwrap_or_default();
                if profile.avatar_url != avatar {
                    profile.avatar_url = avatar;
                }
                if let Some(guild) = msg.guild(&ctx.cache) {
                    if let Some(member) = guild.members.get(&msg.author.id) {
                        let current_nick = member.nick.as_deref().unwrap_or("");
                        if profile.nickname != current_nick {
                            profile.nickname = current_nick.to_string();
                        }
                    }
                }
                let _ = self.profile_store.save(msg.author.id.get(), &profile).await;
            }
        }

        let progress = if user_config.progress_updates_enabled {
            // Check LLM queue utilization so we can show the user their position
            // when the system is saturated (all 4 LLM slots occupied).
            let queue_info = self.agent.llm_queue_info();
            let progress_msg = if queue_info.is_saturated() {
                let position = queue_info.pending + 1;
                format!("⏳ **You are #{position} in line. Waiting for an LLM slot to open up...**")
            } else {
                "🧠 **Thinking...**".to_string()
            };
            reply_no_ping(ctx, msg, &progress_msg).await.ok()
        } else {
            None
        };
        let cancel_token = CancelToken::default();
        if let Some(ref progress) = progress {
            let _ = progress.react(&ctx.http, '❌').await;
            self.progress_messages.lock().await.insert(
                progress.id.get(),
                (msg.author.id.get(), cancel_token.clone()),
            );
        }

        let response_hooks = progress
            .as_ref()
            .map(|progress| ResponseProgressHooks::new(ctx, progress, self.redactor.clone()));

        let user_text = if text.is_empty() {
            "(no text)".to_string()
        } else {
            text
        };
        self.message_log
            .append(msg.author.id.get().to_string(), &user_text)
            .await;
        let user_id_string = msg.author.id.get().to_string();
        let profile_tags = profile
            .tags
            .iter()
            .map(|tag| tag.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let quick_actions = profile
            .quick_actions()
            .into_iter()
            .map(|(name, count)| format!("{name} ({count})"))
            .collect::<Vec<_>>()
            .join(", ");
        let result: AgentResult = self
            .agent
            .run(
                AgentRequest {
                    user_id: &user_id_string,
                    username: &msg.author.name,
                    text: &user_text,
                    media: &media,
                    personality: personality.as_deref(),
                    thinking,
                    channel_id: msg.channel_id.get(),
                    deep_memory_enabled: user_config.deep_memory_enabled && !proactive,
                    display_name: &profile.display_name,
                    nickname: &profile.nickname,
                    avatar_url: &profile.avatar_url,
                    profile_tags: &profile_tags,
                    quick_actions: &quick_actions,
                    guild_id: msg.guild_id.map(|guild| guild.get()),
                    proactive,
                    record_profile_usage: !proactive,
                    max_output_tokens,
                    cancel: Some(cancel_token),
                },
                response_hooks
                    .as_ref()
                    .map_or(&NoHooks as &dyn AgentHooks, |hooks| {
                        hooks as &dyn AgentHooks
                    }),
            )
            .await;

        // ── Cleanup: remove progress message from the cancel registry ──
        let cancelled = if let Some(ref progress) = progress {
            let token_cancelled = self
                .progress_messages
                .lock()
                .await
                .remove(&progress.id.get())
                .is_some_and(|(_, token)| token.is_cancelled());
            let _ = progress
                .delete_reaction(&ctx.http, Some(msg.author.id), '❌')
                .await;
            result.cancelled || token_cancelled
        } else {
            result.cancelled
        };

        // If the user cancelled this request, stop here — no final message.
        if cancelled {
            return;
        }

        {
            let mut convos = self.conversations.lock().await;
            convos.mark_active(
                msg.channel_id.get(),
                msg.author.id.get(),
                Instant::now(),
                followup_timeout,
            );
        }

        // Handle structured development control actions before displaying text.
        if let Some(action) = result.control_action {
            match action {
                AgentControlAction::OwnerConfigurationRequired { job_id } => {
                    self.start_develop_flow(ctx, msg, job_id).await;
                }
                AgentControlAction::OwnerApprovalRequired { job_id } => {
                    // Reply to requester, then DM the owner.
                    self.respond(
                        ctx,
                        msg,
                        "I sent this development request to the bot owner for approval. \
                         Work will not start unless the owner approves it.",
                    )
                    .await;
                    self.notify_owner_for_approval(ctx, msg, job_id).await;
                }
            }
            return;
        }

        let safe = self.redactor.redact(&result.text);
        if user_config.progress_updates_enabled {
            if let Some(notice) = &result.session_notice {
                let _ = reply_no_ping(ctx, msg, notice).await;
            }
        }
        let allowed_pings = extract_mentioned_users(&safe, bot_id.get());
        let with_tool_summary = if user_config.progress_updates_enabled {
            append_tool_summary(&safe, &result.tools_called)
        } else {
            safe
        };
        let (display, code_files) = extract_code_files(&with_tool_summary);
        send_final_message(
            ctx,
            msg,
            &display,
            user_config.labs_pagination_enabled,
            msg.author.id.get(),
            &self.paginated,
            progress.as_ref(),
            &allowed_pings,
        )
        .await;
        // Upload files returned by guarded agent tools.
        for attachment in result.attachments {
            if let Err(error) = msg
                .channel_id
                .send_message(
                    &ctx.http,
                    CreateMessage::new().add_file(CreateAttachment::bytes(
                        attachment.bytes,
                        attachment.filename.clone(),
                    )),
                )
                .await
            {
                tracing::warn!(
                    target: "housebot::files",
                    filename = %attachment.filename,
                    %error,
                    "Failed to send downloaded attachment"
                );
            }
        }
        // Upload extracted code blocks.
        for (filename, content) in code_files {
            let safe = self.redactor.redact(&String::from_utf8_lossy(&content));
            let _ = msg
                .channel_id
                .send_message(
                    &ctx.http,
                    CreateMessage::new()
                        .add_file(CreateAttachment::bytes(safe.into_bytes(), filename)),
                )
                .await;
        }
    }
}
