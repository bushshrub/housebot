//! Token-leaderboard command and the core message-handling flow.

use super::*;

pub(crate) enum ResponseMode {
    Full,
    /// A ping: the classifier may answer it with one reaction. `recent` is
    /// the channel context taken when the message arrived.
    EmojiOrFull {
        recent: Vec<ChannelMessage>,
    },
    /// A full answer nobody asked for: no progress message and no work
    /// thread, so only the reply reaches the channel.
    Unprompted,
}

impl HouseBot {
    pub(crate) async fn handle_token_leaderboard_command(
        &self,
        ctx: &Context,
        cmd: &serenity::all::CommandInteraction,
    ) {
        let user_id = cmd.user.id.get();
        let is_admin = (config::owner_id() != 0 && config::owner_id() == user_id)
            || cmd
                .member
                .as_deref()
                .and_then(|member| member.permissions)
                .is_some_and(|permissions| permissions.administrator());
        let server_config = match cmd.guild_id {
            Some(guild_id) => self.server_cfg.load(guild_id.get()).await,
            None => ServerConfig::default(),
        };
        let access = leaderboard_access(&server_config, cmd.guild_id.is_some(), is_admin);
        let reply = if access == LeaderboardAccess::Denied {
            "This server restricts the token leaderboard to administrators.".into()
        } else {
            let (period, metric) = leaderboard_options(&cmd.data.options);
            self.agent
                .token_leaderboard(period, metric, &user_id.to_string())
                .await
        };
        let reply = self.redactor.redact(&reply);
        let reply = truncate_memory_reply("", &reply);
        let response = CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new()
                .content(reply)
                .ephemeral(access != LeaderboardAccess::Public)
                .allowed_mentions(CreateAllowedMentions::new()),
        );
        if let Err(error) = cmd.create_response(&ctx.http, response).await {
            tracing::warn!(%error, "Failed to send /token_leaderboard response");
        }
    }

    /// The classifier's state document for `msg`, and the bot's name.
    /// `recent` must be taken when `msg` arrived, so the classifier judges
    /// `msg` and not a later message. In a DM nothing is stored, so the state
    /// is the message alone.
    pub(crate) fn classifier_input(
        &self,
        ctx: &Context,
        msg: &Message,
        mut messages: Vec<ChannelMessage>,
    ) -> (String, String) {
        let (bot_id, bot_name) = {
            let bot = ctx.cache.current_user();
            (bot.id.get(), bot.name.clone())
        };
        if messages.is_empty() {
            messages.push(ChannelMessage {
                at: chrono::Utc::now(),
                user_id: msg.author.id.get().to_string(),
                username: msg.author.name.clone(),
                nick: None,
                content: msg.content.clone(),
            });
        }
        (classifier_state(&messages, bot_id, &bot_name), bot_name)
    }

    /// React to `msg` with `emoji`; `false` when Discord refused it.
    pub(crate) async fn react_with(&self, ctx: &Context, msg: &Message, emoji: String) -> bool {
        let reaction = serenity::all::ReactionType::Unicode(emoji.clone());
        match msg.react(&ctx.http, reaction).await {
            Ok(_) => {
                tracing::info!(
                    target: "housebot::emoji",
                    message_id = msg.id.get(),
                    emoji,
                    "Answered with an emoji-only reaction"
                );
                true
            }
            Err(error) => {
                tracing::warn!(
                    target: "housebot::emoji",
                    message_id = msg.id.get(),
                    %error,
                    "Failed to send emoji-only response"
                );
                false
            }
        }
    }

    /// A message in a proactive channel that does not address the bot: the
    /// classifier picks nothing, a reaction, or a full answer to its author.
    pub(crate) async fn handle_proactive(
        &self,
        ctx: &Context,
        msg: &Message,
        bot_id: UserId,
        session_expired: bool,
        followup_timeout: Duration,
        recent: Vec<ChannelMessage>,
    ) {
        let (state, _) = self.classifier_input(ctx, msg, recent);
        match self.agent.classify_proactive(&state).await {
            ProactiveAction::Ignore => tracing::debug!(
                target: "housebot::message_flow",
                message_id = msg.id.get(),
                "Proactive classifier ignored the message"
            ),
            ProactiveAction::React(emoji) => {
                self.react_with(ctx, msg, emoji).await;
            }
            // Another bot answers our reply, which would be escalated again:
            // two bots talking to each other forever.
            ProactiveAction::Escalate if msg.author.bot => tracing::debug!(
                target: "housebot::message_flow",
                message_id = msg.id.get(),
                "Proactive answer skipped: the author is a bot"
            ),
            ProactiveAction::Escalate => {
                // Answers nobody asked for are the costly and noisy outcome,
                // so each channel gets at most one per cooldown.
                if self
                    .proactive_limiter
                    .check(&msg.channel_id.get().to_string())
                {
                    tracing::info!(
                        target: "housebot::message_flow",
                        message_id = msg.id.get(),
                        channel_id = msg.channel_id.get(),
                        "Proactive answer skipped: channel is in cooldown"
                    );
                    return;
                }
                tracing::info!(
                    target: "housebot::message_flow",
                    message_id = msg.id.get(),
                    channel_id = msg.channel_id.get(),
                    "Proactive classifier escalated the message"
                );
                self.handle_message(
                    ctx,
                    msg,
                    bot_id,
                    session_expired,
                    followup_timeout,
                    ResponseMode::Unprompted,
                )
                .await;
            }
        }
    }

    pub(crate) async fn handle_message(
        &self,
        ctx: &Context,
        msg: &Message,
        bot_id: UserId,
        session_expired: bool,
        followup_timeout: Duration,
        response_mode: ResponseMode,
    ) {
        let show_progress = !matches!(response_mode, ResponseMode::Unprompted);
        let mut text = msg.content.clone();
        let role_tokens = bot_role_mentions(ctx, msg, bot_id)
            .into_iter()
            .map(|role_id| format!("<@&{role_id}>"));
        for token in [format!("<@{bot_id}>"), format!("<@!{bot_id}>")]
            .into_iter()
            .chain(role_tokens)
        {
            text = text.replace(&token, "");
        }
        let text = text.trim().to_string();
        let text = match forwarded_message_context(msg) {
            Some(forwarded) if text.is_empty() => forwarded,
            Some(forwarded) => format!("{text}\n\n{forwarded}"),
            None => text,
        };
        let attachment_text = message_attachment_context(msg);
        let text = match attachment_text {
            Some(attachments) if text.is_empty() => attachments,
            Some(attachments) => format!("{text}\n\n{attachments}"),
            None => text,
        };

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
        // A bare ping gives the classifier nothing to judge, and the user
        // expects a reply.
        if let ResponseMode::EmojiOrFull { recent } = response_mode {
            if !text.is_empty() && !message_has_attachments(msg) {
                let (state, bot_name) = self.classifier_input(ctx, msg, recent);
                if let Some(emoji) = self.agent.classify_ping(&state, &bot_name).await {
                    if self.react_with(ctx, msg, emoji).await {
                        return;
                    }
                }
            }
        }
        if session_expired {
            self.agent
                .compact_session(&msg.author.id.get().to_string())
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

        // Load per-user settings (personality and thinking effort).
        let personality = user_config.personality.clone();
        let thinking = user_config.thinking_mode;
        let max_output_tokens = self
            .access
            .load()
            .await
            .policy(msg.author.id.get())
            .max_output_tokens;

        // Sourced live from Discord each turn rather than persisted: the bot no
        // longer keeps a profile store, and these only feed the system prompt.
        let (display_name, avatar_url) = match self.discord.fetch_user(msg.author.id.get()).await {
            Ok(user_info) => (
                user_info.display_name,
                user_info.avatar_url.unwrap_or_default(),
            ),
            Err(_) => (msg.author.name.clone(), String::new()),
        };
        let nickname = msg
            .guild(&ctx.cache)
            .and_then(|guild| {
                guild
                    .members
                    .get(&msg.author.id)
                    .and_then(|m| m.nick.clone())
            })
            .unwrap_or_default();

        // Held until the reply is posted, so the channel shows the bot typing
        // for every part of the turn, even when the progress message fails to send.
        let _typing = TypingIndicator::start(ctx, msg.channel_id);

        // Check LLM scheduler utilization so we can show the user their
        // position when every slot is occupied.
        let scheduler_info = self.agent.llm_scheduler_info();
        let progress_msg = if scheduler_info.is_saturated() {
            let position = scheduler_info.pending + 1;
            format!("⏳ **You are #{position} in line. Waiting for an LLM slot to open up...**")
        } else {
            "🧠 **Thinking...**".to_string()
        };
        let progress = if show_progress {
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

        let user_text = if text.is_empty() {
            "(The user pinged you without any text.)".to_string()
        } else {
            text
        };
        let response_hooks = progress.as_ref().map(|progress| {
            ResponseProgressHooks::new(
                ctx,
                progress,
                work_thread_name(&user_text),
                self.redactor.clone(),
            )
        });
        let user_id_string = msg.author.id.get().to_string();
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
                    display_name: &display_name,
                    nickname: &nickname,
                    avatar_url: &avatar_url,
                    guild_id: msg.guild_id.map(|guild| guild.get()),
                    max_output_tokens,
                    max_tool_rounds: MAX_TOOL_ROUNDS,
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
            let _ = progress.delete_reaction(&ctx.http, None, '❌').await;
            result.cancelled || token_cancelled
        } else {
            result.cancelled
        };

        if let Some(hooks) = &response_hooks {
            hooks.finish(&result.tools_called).await;
        }

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
        if let Some(notice) = &result.session_notice {
            let _ = reply_no_ping(ctx, msg, notice).await;
        }
        let allowed_pings = extract_mentioned_users(&safe, bot_id.get());
        let (display, code_files) = extract_code_files(&safe);
        send_final_message(ctx, msg, &display, &allowed_pings).await;
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
