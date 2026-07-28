//! Slash-command, component, and autocomplete interaction routing.

use super::*;

impl HouseBot {
    pub(super) async fn on_interaction(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Component(component) = &interaction {
            if component.data.custom_id.starts_with(DEVELOP_PREFIX) {
                self.handle_develop_component(&ctx, component).await;
            } else {
                self.handle_pagination_component(&ctx, component).await;
            }
            return;
        }
        if let Interaction::Autocomplete(autocomplete) = &interaction {
            if autocomplete.data.name == "tool_ban" || autocomplete.data.name == "tool_restore" {
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
        if cmd.data.name == "lua" {
            self.handle_lua_command(&ctx, &cmd).await;
            return;
        }
        if cmd.data.name == "token_leaderboard" {
            self.handle_token_leaderboard_command(&ctx, &cmd).await;
            return;
        }
        let reply = match cmd.data.name.as_str() {
            "config" => handle_config_interaction(&self.access, &cmd.data.options, user_id).await,
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
            "tool_restore" => {
                let sub_cmd = cmd.data.options.first().map(|o| o.name.as_str());
                match sub_cmd {
                    Some("propose") => {
                        self.handle_tool_restore_propose(&ctx, &cmd, user_id, guild_id)
                            .await;
                        return;
                    }
                    Some("vote") => {
                        let defer = CreateInteractionResponse::Defer(
                            CreateInteractionResponseMessage::new().ephemeral(true),
                        );
                        if let Err(e) = cmd.create_response(&ctx.http, defer).await {
                            tracing::warn!("Failed to defer /tool_restore vote response: {e}");
                            return;
                        }
                        let reply = self
                            .handle_tool_restore_vote(&ctx, &cmd, user_id, guild_id)
                            .await;
                        let reply = self.redactor.redact(&reply);
                        let _ = cmd
                            .edit_response(&ctx.http, EditInteractionResponse::new().content(reply))
                            .await;
                        return;
                    }
                    _ => {}
                }
                handle_tool_restore_interaction(
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
                    respond_ephemeral(&ctx, &cmd, "No data section specified.").await;
                    return;
                };
                match section.name.as_str() {
                    "profile" => {
                        let Some(actions) = nested_options(section) else {
                            respond_ephemeral(&ctx, &cmd, "No profile action specified.").await;
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
                            respond_ephemeral(&ctx, &cmd, "No history action specified.").await;
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
                                &self.grocery,
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
                    other => {
                        respond_ephemeral(&ctx, &cmd, &format!("Unknown data section `{other}`."))
                            .await;
                        return;
                    }
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
            "skill" => {
                handle_skill_interaction(&self.skills, &self.user_cfg, &cmd.data.options, user_id)
                    .await
            }
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
}
