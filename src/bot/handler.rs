//! Serenity EventHandler: ready, interactions, and messages.

use super::*;

#[serenity::async_trait]
impl EventHandler for HouseBot {
    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!("Logged in as {} (ID: {})", ready.user.name, ready.user.id);
        self.discord.set_http(ctx.http.clone()).await;

        register_slash_commands(&ctx).await;

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

        if self.graph_sweep_started.swap(true, Ordering::SeqCst) {
            return;
        }
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(GRAPH_SWEEP_INTERVAL).await;
                let removed = tokio::task::spawn_blocking(|| {
                    graph_render::sweep_stale_temp_files(&std::env::temp_dir(), GRAPH_SWEEP_MAX_AGE)
                })
                .await
                .unwrap_or(0);
                if removed > 0 {
                    tracing::info!(removed, "Swept stale /lua graph scratch files");
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
                CreateInteractionResponseMessage::new().ephemeral(true),
            );
            if let Err(e) = cmd.create_response(&ctx.http, response).await {
                tracing::warn!("Failed to defer /session compact response: {e}");
                return;
            }
            let hooks = CompactProgressHooks(CompactProgressTarget::Interaction {
                ctx: ctx.clone(),
                command: Box::new(cmd.clone()),
            });
            self.agent
                .compact_session_with_hooks(&user_id.to_string(), deep_memory_enabled, &hooks)
                .await;
            self.conversations
                .lock()
                .await
                .remove(cmd.channel_id.get(), user_id);
            return;
        }
        if cmd.data.name == "lua" {
            self.handle_lua_command(&ctx, &cmd).await;
            return;
        }
        if cmd.data.name == "token_leaderboard" {
            self.handle_token_leaderboard_command(&ctx, &cmd).await;
            return;
        }
        let reply = match cmd.data.name.as_str() {
            "config" => {
                let is_admin = (config::owner_id() != 0 && config::owner_id() == user_id)
                    || cmd
                        .member
                        .as_deref()
                        .and_then(|member| member.permissions)
                        .is_some_and(|permissions| permissions.administrator());
                handle_config_interaction(
                    &self.server_cfg,
                    &self.user_cfg,
                    &cmd.data.options,
                    user_id,
                    guild_id,
                    is_admin,
                )
                .await
            }
            "labs" => handle_labs_interaction(&self.user_cfg, &cmd.data.options, user_id).await,
            "effort" => handle_effort_interaction(&self.user_cfg, &cmd.data.options, user_id).await,
            "tool_ban" => {
                handle_tool_ban_interaction(
                    &self.agent.tool_permissions(),
                    &cmd.data.options,
                    user_id,
                    guild_id,
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
                            .ephemeral(true),
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
                    "profile" => {
                        let Some(actions) = nested_options(section) else {
                            return;
                        };
                        handle_profile_interaction(
                            &self.profile_store,
                            &self.memory,
                            actions,
                            user_id,
                            guild_id,
                        )
                        .await
                    }
                    "history" => {
                        let Some(actions) = nested_options(section) else {
                            return;
                        };
                        handle_history_interaction(
                            &self.history,
                            &self.profile_store,
                            actions,
                            user_id,
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
                                &self.message_log,
                                &self.history,
                                &self.memory,
                                &self.notes,
                                &self.profile_store,
                                &self.user_cfg,
                                &self.agent.reminders().clone(),
                                &self.channel_log,
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
            "storage" => {
                handle_storage_interaction(&self.memory, &self.notes, &cmd.data.options, user_id)
                    .await
            }
            "skill" => handle_skill_interaction(&self.skills, &cmd.data.options, user_id).await,
            "stats" => {
                handle_stats_interaction(
                    &self.history,
                    &self.memory,
                    &self.notes,
                    &self.skills,
                    user_id,
                    cmd.user.display_name(),
                )
                .await
            }
            _ => return,
        };

        let reply = self.redactor.redact(&reply);
        let reply = truncate_memory_reply("", &reply);
        let response = CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new()
                .content(reply)
                .ephemeral(command_response_is_ephemeral(&cmd.data.name)),
        );
        if let Err(e) = cmd.create_response(&ctx.http, response).await {
            tracing::warn!("Failed to send /config response: {e}");
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }
        let content = msg.content.trim().to_string();
        let channel_id = msg.channel_id.get();
        let user_id = msg.author.id.get();

        // ── commands ──
        if msg.content.starts_with("!skill") {
            tracing::info!(target: "housebot::commands", user_id, "!skill command received");
            let (first, rest) = split_command(&msg.content);
            let reply = skill_command(&self.skills, &first, &rest, user_id).await;
            self.respond(&ctx, &msg, &reply).await;
            return;
        }
        if content == "!stats" {
            let reply = stats_command(
                &self.history,
                &self.memory,
                &self.notes,
                &self.skills,
                user_id,
                &msg.author.name,
            )
            .await;
            self.respond(&ctx, &msg, &reply).await;
            return;
        }
        // ── routing ──
        let bot_id = ctx.cache.current_user().id;
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
            self.channel_log
                .append(channel_id, user_id, &msg.author.name, nick, &content)
                .await;
        }

        let is_mentioned = msg.mentions.iter().any(|u| u.id == bot_id);
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

        let proactive = !is_dm
            && user_config.proactive_assistance_enabled
            && !is_mentioned
            && !is_reply_to_bot
            && !is_reply_to_attachment
            && is_proactive_candidate(&content)
            && self.proactive_cooldown_allows(channel_id, user_id).await;
        if !(is_dm
            || is_mentioned
            || is_reply_to_bot
            || is_reply_to_attachment
            || is_active
            || proactive)
        {
            return;
        }
        if self.already_seen(msg.id.get()).await {
            tracing::warn!("Duplicate message {} — skipping", msg.id.get());
            return;
        }

        self.handle_message(
            &ctx,
            &msg,
            bot_id,
            session_expired,
            followup_timeout,
            proactive,
        )
        .await;
        self.mark_done(msg.id.get()).await;
    }
}
