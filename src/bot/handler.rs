//! Serenity EventHandler: ready, interactions, and messages.

use super::message_flow::ResponseMode;
use super::*;
use crate::bot_formatting::format_tokens;

#[serenity::async_trait]
impl EventHandler for HouseBot {
    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!("Logged in as {} (ID: {})", ready.user.name, ready.user.id);
        self.discord.set_http(ctx.http.clone()).await;

        let guild_ids: Vec<GuildId> = ready.guilds.iter().map(|guild| guild.id).collect();
        register_slash_commands(&ctx, &guild_ids).await;

        if self.reminder_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let http = ctx.http.clone();
        let reminders = self.agent.reminders().clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let now = unix_now();
                for r in reminders.pop_due(now).await {
                    if let Ok(uid) = r.user_id.parse::<u64>() {
                        if let Ok(dm) = UserId::new(uid).create_dm_channel(&http).await {
                            let _ = dm
                                .say(&http, format!("⏰ **Reminder:** {}", r.message))
                                .await;
                        }
                    }
                }
            }
        });
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Component(component) = &interaction {
            if component.data.custom_id.starts_with(DEVELOP_PREFIX) {
                self.handle_develop_component(&ctx, component).await;
            }
            return;
        }
        if let Interaction::Autocomplete(_) = &interaction {
            return;
        }
        let Interaction::Command(cmd) = interaction else {
            return;
        };
        let user_id = cmd.user.id.get();
        let guild_id = cmd.guild_id.map(|g| g.get());
        tracing::info!(
            target: "housebot::commands",
            user_id,
            command = %cmd.data.name,
            "Slash command received"
        );
        let session_action = cmd.data.options.first().map(|option| option.name.as_str());
        if cmd.data.name == "session" && session_action == Some("compact") {
            let response = CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(false),
            );
            if let Err(e) = cmd.create_response(&ctx.http, response).await {
                tracing::warn!("Failed to defer /session compact response: {e}");
                return;
            }
            let hooks = CompactProgressHooks::new(ctx.clone(), Box::new(cmd.clone()));
            let compacted = self
                .agent
                .compact_session_with_hooks(&user_id.to_string(), &hooks)
                .await;
            if compacted {
                self.conversations
                    .lock()
                    .await
                    .remove(cmd.channel_id.get(), user_id);
            }
            return;
        }
        if cmd.data.name == "token_leaderboard" {
            self.handle_token_leaderboard_command(&ctx, &cmd).await;
            return;
        }
        let reply = match cmd.data.name.as_str() {
            "config" => {
                handle_config_interaction(
                    &self.access,
                    self.agent.llm_scheduler(),
                    &self.agent.scheduler_limits(),
                    &cmd.data.options,
                    user_id,
                )
                .await
            }
            "server-config" => {
                let is_server_admin = cmd
                    .member
                    .as_deref()
                    .and_then(|member| member.permissions)
                    .is_some_and(|permissions| permissions.administrator());
                let authorized = is_server_admin
                    || self
                        .access
                        .load()
                        .await
                        .is_configurer(user_id, config::owner_id());
                handle_server_config_interaction(
                    &self.server_cfg,
                    &cmd.data.options,
                    guild_id,
                    authorized,
                )
                .await
            }
            "personalize" => {
                handle_personalize_interaction(&self.user_cfg, &cmd.data.options, user_id).await
            }
            "labs" => {
                let caller = LabsCaller {
                    guild_id,
                    is_server_admin: cmd
                        .member
                        .as_deref()
                        .and_then(|member| member.permissions)
                        .is_some_and(|permissions| permissions.administrator()),
                    is_configurer: self
                        .access
                        .load()
                        .await
                        .is_configurer(user_id, config::owner_id()),
                };
                handle_labs_interaction(&self.agent, &self.server_cfg, &cmd.data.options, caller)
                    .await
            }
            "effort" => {
                let is_server_admin = cmd
                    .member
                    .as_deref()
                    .and_then(|member| member.permissions)
                    .is_some_and(|permissions| permissions.administrator());
                let is_configurer = self
                    .access
                    .load()
                    .await
                    .is_configurer(user_id, config::owner_id());
                handle_effort_interaction(
                    &self.user_cfg,
                    &cmd.data.options,
                    user_id,
                    is_server_admin || is_configurer,
                )
                .await
            }
            "status" => handle_status_interaction(&self.user_cfg, user_id).await,
            "help" => help_response(),
            "commit" => commit_hash_response(option_env!("HOUSEBOT_GIT_SHA")),
            "model" => self.agent.model_info(),
            "session" => {
                if session_action == Some("new") {
                    self.handle_new(cmd.channel_id.get(), user_id).await
                } else {
                    let info = self.agent.session_info(&user_id.to_string()).await;
                    let percent = info.context_tokens as f64
                        / info.context_window_tokens.max(1) as f64
                        * 100.0;
                    let response = CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .embed(
                                CreateEmbed::new()
                                    .title("Session")
                                    .field(
                                        "Context",
                                        format!(
                                            "{} / {} tokens ({percent:.1}%)",
                                            format_tokens(info.context_tokens as u64),
                                            format_tokens(info.context_window_tokens as u64)
                                        ),
                                        true,
                                    )
                                    .field("Messages", info.messages.to_string(), true)
                                    .field("Model requests", info.requests.to_string(), true)
                                    .field("Input tokens", format_tokens(info.input_tokens), true)
                                    .field("Output tokens", format_tokens(info.output_tokens), true)
                                    .field(
                                        "Cached tokens",
                                        format_tokens(info.cached_tokens),
                                        true,
                                    ),
                            )
                            .ephemeral(false),
                    );
                    if let Err(e) = cmd.create_response(&ctx.http, response).await {
                        tracing::warn!("Failed to send /session response: {e}");
                    }
                    return;
                }
            }
            "data" => {
                let Some(section) = cmd.data.options.first() else {
                    return;
                };
                match section.name.as_str() {
                    "history" => {
                        let Some(actions) = nested_options(section) else {
                            return;
                        };
                        handle_history_interaction(
                            &self.history,
                            actions,
                            user_id,
                            &cmd.user.name,
                            guild_id,
                        )
                        .await
                    }
                    "erase" => {
                        let options = nested_options(section).unwrap_or_default();
                        if bool_option(options, "confirm") != Some(true) {
                            "Nothing was erased. Set `confirm:true` only when you want to permanently delete all stored data.".into()
                        } else {
                            let reply = erase_data_command(
                                &self.history,
                                &self.memory,
                                &self.user_cfg,
                                &self.agent.reminders().clone(),
                                &self.channel_context,
                                user_id,
                            )
                            .await;
                            self.agent.reset_session(&user_id.to_string()).await;
                            self.agent.clear_token_data(&user_id.to_string()).await;
                            self.conversations
                                .lock()
                                .await
                                .remove(cmd.channel_id.get(), user_id);
                            reply
                        }
                    }
                    _ => return,
                }
            }
            "storage" => handle_storage_interaction(&self.memory, &cmd.data.options, user_id).await,
            "skill" => handle_skill_interaction(&self.skills, &cmd.data.options, user_id).await,
            "stats" => {
                handle_stats_interaction(
                    &self.history,
                    &self.memory,
                    &self.skills,
                    user_id,
                    cmd.user.display_name(),
                    &self.agent.user_token_summary(&user_id.to_string()).await,
                )
                .await
            }
            _ => return,
        };

        let reply = self.redactor.redact(&reply);
        let message = CreateInteractionResponseMessage::new()
            .ephemeral(command_response_is_ephemeral(&cmd.data.name));
        // An embed description holds twice what message content does, so a
        // long reply (/help is the one that reaches this) survives intact.
        let message = if reply.chars().count() > MAX_MESSAGE_LENGTH {
            message.embed(CreateEmbed::new().description(truncate_reply(
                "",
                &reply,
                EMBED_DESCRIPTION_LIMIT,
            )))
        } else {
            message.content(reply)
        };
        let response = CreateInteractionResponse::Message(message);
        if let Err(e) = cmd.create_response(&ctx.http, response).await {
            tracing::warn!("Failed to send /config response: {e}");
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        let bot_id = ctx.cache.current_user().id;
        if msg.author.id == bot_id {
            // Never respond to our own messages, e.g. a reply chain off our
            // own "Thinking..." progress updates would otherwise loop forever.
            return;
        }
        let structured_mention = msg.mentions.iter().any(|u| u.id == bot_id);
        let raw_mention = content_mentions_user(&msg.content, bot_id.get());
        let role_mention = !bot_role_mentions(&ctx, &msg, bot_id).is_empty();
        let is_mentioned = structured_mention || raw_mention || role_mention;
        if msg.author.bot {
            // Other bots get through only where the server allows bot
            // interactions. An unmentioned bot message then reaches only the
            // proactive classifier, never a reply or follow-up, so two bots
            // cannot answer each other in a loop.
            let respond = if let Some(gid) = msg.guild_id {
                self.server_cfg.load(gid.get()).await.respond_to_bot_pings
            } else {
                false
            };
            if !respond {
                if is_mentioned {
                    tracing::info!(
                        target: "housebot::message_flow",
                        message_id = msg.id.get(),
                        author_id = msg.author.id.get(),
                        "Dropped message: bot mentions are disabled for this server"
                    );
                } else {
                    tracing::debug!(
                        target: "housebot::message_flow",
                        message_id = msg.id.get(),
                        author_id = msg.author.id.get(),
                        "Dropped message: bot author did not mention us"
                    );
                }
                return;
            }
        }
        if msg.author.bot && is_mentioned {
            tracing::info!(
                target: "housebot::bot_mentions",
                author_id = msg.author.id.get(),
                guild_id = msg.guild_id.map(|id| id.get()),
                channel_id = msg.channel_id.get(),
                structured_mention,
                raw_mention,
                "Accepted explicit mention from another bot"
            );
        }
        let content = msg.content.trim().to_string();
        let channel_id = msg.channel_id.get();
        let user_id = msg.author.id.get();
        let is_dm = msg.guild_id.is_none();
        let guild_id = msg.guild_id.map(|g| g.get());
        let is_reply_to_bot = msg
            .referenced_message
            .as_ref()
            .map(|m| m.author.id == bot_id)
            .unwrap_or(false);

        // Configurers (and the owner) always get through; other users can be
        // silenced entirely by a configurer-set policy.
        let access = self.access.load().await;
        if !access.should_respond(user_id, config::owner_id()) {
            // The notice is public (only interactions can be ephemeral), so the
            // cooldown keeps a blocked user from using it to spam the channel.
            if (is_dm || is_mentioned || is_reply_to_bot)
                && self.allowed_config_channel(&ctx, &msg).await.is_some()
                && !self.blocked_notice_limiter.check(&user_id.to_string())
            {
                self.respond(&ctx, &msg, BLOCKED_USER_NOTICE).await;
            }
            tracing::info!(
                target: "housebot::message_flow",
                message_id = msg.id.get(),
                user_id,
                "Dropped message: access policy silences this user"
            );
            return;
        }

        // ── commands ──
        if msg.content.starts_with("!skill") {
            tracing::info!(target: "housebot::commands", user_id, "!skill command received");
            let (first, _rest) = split_command(&msg.content);
            let reply = skill_command(&self.skills, &first, user_id).await;
            let reply = self.redactor.redact(&reply);
            self.respond(&ctx, &msg, &reply).await;
            return;
        }
        if content == "!stats" {
            let reply = stats_command(
                &self.history,
                &self.memory,
                &self.skills,
                user_id,
                &msg.author.name,
            )
            .await;
            self.respond(&ctx, &msg, &reply).await;
            return;
        }
        // ── routing ──
        // Check channel allowlist before doing anything else.
        let Some(config_channel_id) = self.allowed_config_channel(&ctx, &msg).await else {
            if is_mentioned {
                tracing::info!(
                    target: "housebot::message_flow",
                    message_id = msg.id.get(),
                    user_id,
                    channel_id,
                    "Dropped message: channel is not allowed"
                );
            } else {
                tracing::debug!(
                    target: "housebot::message_flow",
                    message_id = msg.id.get(),
                    channel_id,
                    "Dropped message: channel is not allowed"
                );
            }
            return;
        };

        // Taken in the same step as the append, so a message that arrives
        // during the awaits below cannot become the one the classifier judges.
        let mut recent = Vec::new();
        if !is_dm {
            // Prefer server nickname, then global display name, over the raw username.
            let nick = msg
                .member
                .as_ref()
                .and_then(|m| m.nick.as_deref())
                .or(msg.author.global_name.as_deref())
                .filter(|n| *n != msg.author.name);
            self.channel_context
                .append(channel_id, user_id, &msg.author.name, nick, &content);
            recent = self
                .channel_context
                .recent(channel_id, CLASSIFIER_CONTEXT_MESSAGES);
        }

        let is_reply_to_attachment = msg
            .referenced_message
            .as_deref()
            .is_some_and(message_has_attachments);

        // Unpinged follow-ups only happen in DMs.
        let followup_timeout =
            Duration::from_secs(config::env_parse("CONVERSATION_IDLE_TIMEOUT", 300));

        let now = Instant::now();
        let (is_active, session_expired) = {
            let mut convos = self.conversations.lock().await;
            let active = is_dm && convos.is_active(channel_id, user_id, now);
            let expired = !active && convos.pop_timed_out(channel_id, user_id, now);
            (active, expired)
        };

        let addressed = (!msg.author.bot || is_mentioned)
            && (is_dm || is_mentioned || is_reply_to_bot || is_reply_to_attachment || is_active);
        let proactive = match guild_id {
            Some(gid) if !addressed => {
                let channels = self.server_cfg.load(gid).await.proactive_channel_ids;
                // With no allowlist, `config_channel_id` is the thread itself,
                // so a thread is also matched through its parent.
                channels.contains(&config_channel_id)
                    || !channels.is_empty()
                        && thread_parent_id(&ctx, &msg)
                            .await
                            .is_some_and(|parent| channels.contains(&parent))
            }
            _ => false,
        };
        if !addressed && !proactive {
            tracing::debug!(
                target: "housebot::message_flow",
                message_id = msg.id.get(),
                channel_id,
                "Dropped message: not addressed to the bot"
            );
            return;
        }
        if self.already_seen(msg.id.get()).await {
            tracing::warn!(
                target: "housebot::message_flow",
                message_id = msg.id.get(),
                "Dropped message: duplicate"
            );
            return;
        }

        if proactive {
            self.handle_proactive(
                &ctx,
                &msg,
                bot_id,
                session_expired,
                followup_timeout,
                recent,
            )
            .await;
        } else {
            let response_mode = if is_mentioned && !is_reply_to_bot && !is_reply_to_attachment {
                ResponseMode::EmojiOrFull { recent }
            } else {
                ResponseMode::Full
            };
            self.handle_message(
                &ctx,
                &msg,
                bot_id,
                session_expired,
                followup_timeout,
                response_mode,
            )
            .await;
        }
        self.mark_done(msg.id.get()).await;
    }

    async fn reaction_add(&self, ctx: Context, reaction: serenity::all::Reaction) {
        let user_id = match reaction.user_id {
            Some(id) => id.get(),
            None => return,
        };
        let bot_id = ctx.cache.current_user().id.get();
        if user_id == bot_id {
            return;
        }

        // ── Cancel reaction: ❌ on a progress message ────────────────────
        if let serenity::all::ReactionType::Unicode(e) = &reaction.emoji {
            if e == "❌" {
                let progress = self
                    .progress_messages
                    .lock()
                    .await
                    .get(&reaction.message_id.get())
                    .cloned();
                if let Some((owner_id, cancel_token)) = progress {
                    if owner_id == user_id {
                        cancel_token.cancel();
                        let _ = reaction
                            .channel_id
                            .edit_message(
                                &ctx.http,
                                reaction.message_id,
                                EditMessage::new().content("❌ **Cancelled**"),
                            )
                            .await;
                        let _ = reaction
                            .channel_id
                            .delete_reaction(
                                &ctx.http,
                                reaction.message_id,
                                Some(UserId::new(user_id)),
                                '❌',
                            )
                            .await;
                        return;
                    }
                }
            }
        }

        // ── Emoji echo: when a user reacts to a bot reply, copy the reaction
        //    back to the user's original message.
        //
        //    We do this *before* the tool-ban check so that the message-fetch
        //    is shared: the tool-ban path returns early on non-proposal
        //    messages, which is *after* our echo has already fired.
        if let Ok(message) = reaction
            .channel_id
            .message(&ctx.http, reaction.message_id)
            .await
        {
            if message.author.id.get() == bot_id {
                if let Some(ref referenced) = message.referenced_message {
                    let _ = referenced.react(&ctx.http, reaction.emoji.clone()).await;
                }
            }
        }
    }
}
