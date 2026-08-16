//! Serenity EventHandler: ready, interactions, and messages.

use super::message_flow::ResponseMode;
use super::*;

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
            } else {
                self.handle_pagination_component(&ctx, component).await;
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
            let deep_memory_enabled = self.user_cfg.load(user_id).await.deep_memory_enabled;
            let response = CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(false),
            );
            if let Err(e) = cmd.create_response(&ctx.http, response).await {
                tracing::warn!("Failed to defer /session compact response: {e}");
                return;
            }
            let hooks = CompactProgressHooks::new(ctx.clone(), Box::new(cmd.clone()));
            self.agent
                .compact_session_with_hooks(&user_id.to_string(), deep_memory_enabled, &hooks)
                .await;
            self.conversations
                .lock()
                .await
                .remove(cmd.channel_id.get(), user_id);
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
                handle_personalize_interaction(
                    &self.user_cfg,
                    &cmd.data.options,
                    user_id,
                    is_server_admin || is_configurer,
                )
                .await
            }
            "labs" => handle_labs_interaction(&self.user_cfg, &cmd.data.options, user_id).await,
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
                                            info.context_tokens, info.context_window_tokens
                                        ),
                                        true,
                                    )
                                    .field("Messages", info.messages.to_string(), true)
                                    .field("Model requests", info.requests.to_string(), true)
                                    .field("Input tokens", info.input_tokens.to_string(), true)
                                    .field("Output tokens", info.output_tokens.to_string(), true)
                                    .field("Cached tokens", info.cached_tokens.to_string(), true),
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
            "privacy" => {
                handle_privacy_interaction(&self.user_cfg, &self.memory, &cmd.data.options, user_id)
                    .await
            }
            "storage" => handle_storage_interaction(&self.memory, &cmd.data.options, user_id).await,
            "skill" => {
                handle_skill_interaction(&self.skills, &self.user_cfg, &cmd.data.options, user_id)
                    .await
            }
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
        if msg.webhook_id.is_some() && self.handle_dev_notify_webhook(&ctx, &msg).await {
            // Only short-circuit for the configured dev-notify channel; other
            // webhook messages (e.g. from other bots) still flow through the
            // normal pipeline below, same as before this feature existed.
            return;
        }
        let structured_mention = msg.mentions.iter().any(|u| u.id == bot_id);
        let raw_mention = content_mentions_user(&msg.content, bot_id.get());
        let is_mentioned = structured_mention || raw_mention;
        if msg.author.bot {
            // Other bots must explicitly @-mention us; unmentioned bot
            // messages are always ignored regardless of configuration.
            if !is_mentioned {
                return;
            }
            let respond = if let Some(gid) = msg.guild_id {
                self.server_cfg.load(gid.get()).await.respond_to_bot_pings
            } else {
                false
            };
            if !respond {
                return;
            }
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

        // Configurers (and the owner) always get through; other users can be
        // silenced entirely by a configurer-set policy.
        let access = self.access.load().await;
        if !access.should_respond(user_id, config::owner_id()) {
            return;
        }

        // ── commands ──
        if msg.content.starts_with("!skill") {
            tracing::info!(target: "housebot::commands", user_id, "!skill command received");
            let (first, _rest) = split_command(&msg.content);
            let reply = skill_command(&self.skills, &self.user_cfg, &first, user_id).await;
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
        let is_dm = msg.guild_id.is_none();
        let guild_id = msg.guild_id.map(|g| g.get());

        // Check channel allowlist before doing anything else.
        if !self
            .server_cfg
            .is_channel_allowed(guild_id, channel_id)
            .await
        {
            return;
        }

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
        }

        let is_reply_to_bot = msg
            .referenced_message
            .as_ref()
            .map(|m| m.author.id == bot_id)
            .unwrap_or(false);
        let is_reply_to_attachment = msg
            .referenced_message
            .as_deref()
            .is_some_and(message_has_attachments);

        // Follow-ups are on by default in DMs. In guild channels, users must
        // opt in and the channel must be explicitly configured by the server.
        let user_config = self.user_cfg.load(user_id).await;
        let followup_enabled = is_dm || user_config.followup_enabled;
        let followup_timeout = Duration::from_secs(user_config.followup_timeout_secs);
        let followup_channel_allowed = self
            .server_cfg
            .is_followup_channel_allowed(guild_id, channel_id)
            .await;
        let followup_channel_allowed = is_dm || followup_channel_allowed;

        let now = Instant::now();
        let (is_active, session_expired) = {
            let mut convos = self.conversations.lock().await;
            let active = followup_enabled
                && followup_channel_allowed
                && convos.is_active(channel_id, user_id, now);
            let expired = !active && convos.pop_timed_out(channel_id, user_id, now);
            (active, expired)
        };

        if !(is_dm || is_mentioned || is_reply_to_bot || is_reply_to_attachment || is_active) {
            return;
        }
        if self.already_seen(msg.id.get()).await {
            tracing::warn!("Duplicate message {} — skipping", msg.id.get());
            return;
        }

        let response_mode = if is_mentioned && !is_reply_to_bot && !is_reply_to_attachment {
            ResponseMode::EmojiOrFull
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
