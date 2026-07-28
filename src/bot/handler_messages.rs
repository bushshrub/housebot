//! Gateway `ready` and inbound-message handling.

use super::message_flow::ResponseMode;
use super::*;

impl HouseBot {
    pub(super) async fn on_ready(&self, ctx: Context, ready: Ready) {
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

    pub(super) async fn on_message(&self, ctx: Context, msg: Message) {
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
        let is_dm = msg.guild_id.is_none();
        let guild_id = msg.guild_id.map(|g| g.get());

        // Configurers (and the owner) always get through; other users can be
        // silenced entirely by a configurer-set policy.
        let access = self.access.load().await;
        if !access.should_respond(user_id, config::owner_id()) {
            return;
        }

        // ── channel allowlist (before any command) ──
        if !self
            .server_cfg
            .is_channel_allowed(guild_id, channel_id)
            .await
        {
            return;
        }

        // ── commands ──
        if content.starts_with("!skill") {
            tracing::info!(target: "housebot::commands", user_id, "!skill command received");
            let (first, _rest) = split_command(&msg.content);
            let reply = skill_command(&self.skills, &self.user_cfg, &first, user_id).await;
            let reply = self.redactor.redact(&reply);
            self.respond(&ctx, &msg, &reply).await;
            return;
        }
        if content.starts_with("!grocery") {
            tracing::info!(target: "housebot::commands", user_id, "!grocery command received");
            let (first, rest) = split_command(&msg.content);
            let reply = grocery_command(&self.grocery, &first, &rest, user_id).await;
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
            && access.proactive_enabled
            && user_config.proactive_assistance_enabled
            && !is_mentioned
            && !is_reply_to_bot
            && !is_reply_to_attachment
            && is_proactive_candidate(&content)
            && self.server_proactive_allowed(guild_id).await
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

        let response_mode = if is_mentioned && !is_reply_to_bot && !is_reply_to_attachment {
            ResponseMode::EmojiOrFull
        } else {
            ResponseMode::Full { proactive }
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
}
