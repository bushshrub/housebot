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
        if let Interaction::Autocomplete(autocomplete) = &interaction {
            if autocomplete.data.name == "tool_ban" {
                Self::handle_tool_ban_autocomplete(&ctx, autocomplete).await;
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
                let sub_cmd = cmd.data.options.first().map(|o| o.name.as_str());
                match sub_cmd {
                    Some("propose") => {
                        self.handle_tool_ban_propose(&ctx, &cmd, user_id, guild_id)
                            .await;
                        return;
                    }
                    Some("vote") => {
                        let reply = self
                            .handle_tool_ban_vote(&ctx, &cmd, user_id, guild_id)
                            .await;
                        let reply = self.redactor.redact(&reply);
                        let response = CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new()
                                .content(reply)
                                .ephemeral(true),
                        );
                        if let Err(e) = cmd.create_response(&ctx.http, response).await {
                            tracing::warn!("Failed to send /tool_ban vote response: {e}");
                        }
                        return;
                    }
                    _ => {}
                }
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

    async fn reaction_add(&self, ctx: Context, reaction: serenity::all::Reaction) {
        let user_id = match reaction.user_id {
            Some(id) => id.get(),
            None => return,
        };
        let bot_id = ctx.cache.current_user().id.get();
        if user_id == bot_id {
            return;
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

        // ── Tool-ban voting ──────────────────────────────────────────────
        let Some(guild_id) = reaction.guild_id.map(|g| g.get()) else {
            return;
        };
        let message_id = reaction.message_id.get();
        let approve = match &reaction.emoji {
            serenity::all::ReactionType::Unicode(e) if e == "\u{2705}" => true,
            serenity::all::ReactionType::Unicode(e) if e == "\u{274C}" => false,
            _ => return,
        };

        let permissions = self.agent.tool_permissions();
        let found = match permissions.find_by_message(message_id).await {
            Ok(found) => found,
            Err(error) => {
                tracing::error!(%error, %message_id, "Failed to load proposals for reaction vote");
                return;
            }
        };
        let Some((_id, proposal)) = found else {
            return;
        };
        if proposal.guild_id != guild_id {
            return;
        }

        match permissions
            .vote(guild_id, &proposal.id, user_id, approve)
            .await
        {
            Ok(VoteResult::Pending {
                approvals,
                rejections,
                quorum,
            }) => {
                let text = self.redactor.redact(&format_proposal_message(
                    &proposal, approvals, rejections, quorum,
                ));
                let _ = ChannelId::new(proposal.channel_id)
                    .edit_message(&ctx.http, message_id, EditMessage::new().content(text))
                    .await;
            }
            Ok(VoteResult::Approved(ref ban)) => {
                let text = self.redactor.redact(&format_approved_message(ban));
                let _ = ChannelId::new(proposal.channel_id)
                    .edit_message(&ctx.http, message_id, EditMessage::new().content(text))
                    .await;
            }
            Ok(VoteResult::Rejected) => {
                let text = self.redactor.redact(&format_rejected_message(&proposal));
                let _ = ChannelId::new(proposal.channel_id)
                    .edit_message(&ctx.http, message_id, EditMessage::new().content(text))
                    .await;
            }
            Err(error) => {
                tracing::debug!(%error, %user_id, %message_id, "Reaction vote failed");
            }
        }
    }
}

// ── tool_ban autocomplete ────────────────────────────────────────────────────

impl HouseBot {
    /// Respond to autocomplete for `/tool_ban propose tool:`.
    async fn handle_tool_ban_autocomplete(
        ctx: &Context,
        autocomplete: &serenity::all::CommandInteraction,
    ) {
        let Some(focused) = autocomplete.data.autocomplete() else {
            return;
        };
        if focused.name != "tool" {
            return;
        }
        let partial = focused.value;
        let lower = partial.to_ascii_lowercase();
        let mut names: Vec<&str> = crate::tools::all_tool_names()
            .iter()
            .filter(|name| name.contains(&lower))
            .copied()
            .take(25)
            .collect();
        names.sort_unstable();
        let mut resp = CreateAutocompleteResponse::new();
        for name in names {
            resp = resp.add_string_choice(name, name);
        }
        let _ = autocomplete
            .create_response(&ctx.http, CreateInteractionResponse::Autocomplete(resp))
            .await;
    }

    /// Handle `/tool_ban vote`: record the vote and update the public proposal message.
    async fn handle_tool_ban_vote(
        &self,
        ctx: &Context,
        cmd: &serenity::all::CommandInteraction,
        author_id: u64,
        guild_id: Option<u64>,
    ) -> String {
        let Some(guild_id) = guild_id else {
            return "Tool-ban voting is only available inside a server.".into();
        };
        let Some(option) = cmd.data.options.first() else {
            return "Unexpected option structure.".into();
        };
        let CommandDataOptionValue::SubCommand(options) = &option.value else {
            return "Unexpected option structure.".into();
        };
        let proposal_str = options
            .iter()
            .find(|option| option.name == "proposal")
            .and_then(|option| match &option.value {
                CommandDataOptionValue::String(id) => Some(id.as_str()),
                _ => None,
            });
        let approve = options
            .iter()
            .find(|option| option.name == "approve")
            .and_then(|option| match option.value {
                CommandDataOptionValue::Boolean(approve) => Some(approve),
                _ => None,
            });
        let (Some(proposal_str), Some(approve)) = (proposal_str, approve) else {
            return "Please specify a proposal ID and vote.".into();
        };

        let permissions = self.agent.tool_permissions();

        // Look up the proposal *before* voting so we have channel/message IDs
        // even if the vote finalizes and removes the proposal.
        let proposal_info = permissions
            .find_proposal_by_prefix(guild_id, proposal_str)
            .await
            .unwrap_or(None);

        match permissions
            .vote(guild_id, proposal_str, author_id, approve)
            .await
        {
            Ok(VoteResult::Pending {
                approvals,
                rejections,
                quorum,
            }) => {
                // Update the public message if we have channel/message IDs.
                if let Some(ref p) = proposal_info {
                    if p.channel_id != 0 && p.message_id != 0 {
                        let text = self
                            .redactor
                            .redact(&format_proposal_message(p, approvals, rejections, quorum));
                        let _ = ChannelId::new(p.channel_id)
                            .edit_message(&ctx.http, p.message_id, EditMessage::new().content(text))
                            .await;
                    }
                }
                format!(
                    "✅ Vote recorded. Current result: **{approvals} approve / {rejections} reject** (minimum {quorum} votes)."
                )
            }
            Ok(VoteResult::Approved(ref ban)) => {
                // Update the public message if we have channel/message IDs.
                if let Some(ref p) = proposal_info {
                    if p.channel_id != 0 && p.message_id != 0 {
                        let text = self.redactor.redact(&format_approved_message(ban));
                        let _ = ChannelId::new(p.channel_id)
                            .edit_message(&ctx.http, p.message_id, EditMessage::new().content(text))
                            .await;
                    }
                }
                format!(
                    "🚫 Vote passed. <@{}> is now blocked from using `{}` in this server.",
                    ban.user_id, ban.tool_name
                )
            }
            Ok(VoteResult::Rejected) => {
                // Update the public message if we have channel/message IDs.
                if let Some(ref p) = proposal_info {
                    if p.channel_id != 0 && p.message_id != 0 {
                        let text = self.redactor.redact(&format_rejected_message(p));
                        let _ = ChannelId::new(p.channel_id)
                            .edit_message(&ctx.http, p.message_id, EditMessage::new().content(text))
                            .await;
                    }
                }
                "✅ The proposal was rejected by majority vote.".into()
            }
            Err(error) => format!("⚠️ {error}"),
        }
    }

    /// Handle `/tool_ban propose`: send a visible channel message and add emoji
    /// voting reactions, then respond to the interaction ephemerally.
    async fn handle_tool_ban_propose(
        &self,
        ctx: &Context,
        cmd: &serenity::all::CommandInteraction,
        author_id: u64,
        guild_id: Option<u64>,
    ) {
        let Some(guild_id) = guild_id else {
            respond_ephemeral(
                ctx,
                cmd,
                "Tool-ban voting is only available inside a server.",
            )
            .await;
            return;
        };
        let Some(option) = cmd.data.options.first() else {
            respond_ephemeral(ctx, cmd, "Unexpected option structure.").await;
            return;
        };
        let CommandDataOptionValue::SubCommand(options) = &option.value else {
            respond_ephemeral(ctx, cmd, "Unexpected option structure.").await;
            return;
        };
        let target = options
            .iter()
            .find(|option| option.name == "user")
            .and_then(|option| match option.value {
                CommandDataOptionValue::User(user) => Some(user.get()),
                _ => None,
            });
        let tool = options
            .iter()
            .find(|option| option.name == "tool")
            .and_then(|option| match &option.value {
                CommandDataOptionValue::String(tool) => Some(tool.as_str()),
                _ => None,
            });
        let (Some(target), Some(tool)) = (target, tool) else {
            respond_ephemeral(ctx, cmd, "Please specify both a user and tool name.").await;
            return;
        };

        // Defer the interaction so we have time to post the channel message.
        let defer = CreateInteractionResponse::Defer(
            CreateInteractionResponseMessage::new().ephemeral(true),
        );
        if let Err(e) = cmd.create_response(&ctx.http, defer).await {
            tracing::warn!("Failed to defer /tool_ban propose response: {e}");
            return;
        }

        let permissions = self.agent.tool_permissions();
        let proposal = match permissions.propose(guild_id, target, tool, author_id).await {
            Ok(p) => p,
            Err(error) => {
                let _ = cmd
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new().content(format!("⚠️ {error}")),
                    )
                    .await;
                return;
            }
        };

        // Send a visible channel message with the proposal.
        let (approvals, _) = proposal.vote_counts();
        let text = self.redactor.redact(&format_proposal_message(
            &proposal,
            approvals,
            0,
            permissions.min_votes(),
        ));
        let msg = match cmd
            .channel_id
            .send_message(&ctx.http, CreateMessage::new().content(text))
            .await
        {
            Ok(msg) => msg,
            Err(error) => {
                tracing::warn!(%error, "Failed to send proposal channel message");
                // Roll back the proposal so we don't orphan it.
                if let Err(e) = permissions.remove_proposal(guild_id, &proposal.id).await {
                    tracing::error!(%e, "Failed to roll back proposal after message send failure");
                }
                let _ = cmd
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new()
                            .content("⚠️ Failed to post proposal to channel."),
                    )
                    .await;
                return;
            }
        };

        // Store the message info in the proposal.
        if let Err(error) = permissions
            .set_proposal_message(guild_id, &proposal.id, cmd.channel_id.get(), msg.id.get())
            .await
        {
            tracing::error!(%error, "Failed to store proposal message IDs — deleting posted message");
            // Remove the orphaned message since we can't track it.
            let _ = msg.delete(&ctx.http).await;
            if let Err(e) = permissions.remove_proposal(guild_id, &proposal.id).await {
                tracing::error!(%e, "Failed to roll back proposal after message mapping failure");
            }
            let _ = cmd
                .edit_response(
                    &ctx.http,
                    EditInteractionResponse::new()
                        .content("⚠️ Failed to save proposal metadata. Please try again."),
                )
                .await;
            return;
        }

        // Add voting reactions.
        let _ = msg
            .react(
                &ctx.http,
                serenity::all::ReactionType::Unicode("\u{2705}".to_string()),
            )
            .await;
        let _ = msg
            .react(
                &ctx.http,
                serenity::all::ReactionType::Unicode("\u{274C}".to_string()),
            )
            .await;

        // Edit the deferred response with a confirmation.
        let confirmation = self.redactor.redact(&format!(
            "✅ Proposal created! Everyone in the server can see it and vote with reactions. \
             Proposal ID: `{}`. Vote also with `/tool_ban vote proposal:{} approve:true|false`.",
            &proposal.id[..8],
            &proposal.id[..8],
        ));
        let _ = cmd
            .edit_response(
                &ctx.http,
                EditInteractionResponse::new().content(confirmation),
            )
            .await;
    }
}

// ── Proposal message formatting helpers ──────────────────────────────────────

fn format_proposal_message(
    proposal: &crate::tool_permissions::BanProposal,
    approvals: usize,
    rejections: usize,
    min_votes: usize,
) -> String {
    format!(
        "🗳️ **Ban proposal** by <@{}>\n\
         Target: <@{}>\n\
         Tool: `{}`\n\
         Votes: **{} approve** / **{} reject** (minimum {} votes)\n\
         React with ✅ to approve, ❌ to reject (or use `/tool_ban vote`)\n\
         Proposal ID: `{}`",
        proposal.proposed_by,
        proposal.target_user_id,
        proposal.tool_name,
        approvals,
        rejections,
        min_votes,
        &proposal.id[..8],
    )
}

fn format_approved_message(ban: &crate::tool_permissions::ToolBan) -> String {
    format!(
        "🚫 **Ban approved!** <@{}> is now blocked from using `{}`.",
        ban.user_id, ban.tool_name
    )
}

fn format_rejected_message(proposal: &crate::tool_permissions::BanProposal) -> String {
    format!(
        "❌ **Ban rejected.** The proposal to restrict <@{}> from `{}` did not pass.",
        proposal.target_user_id, proposal.tool_name
    )
}
