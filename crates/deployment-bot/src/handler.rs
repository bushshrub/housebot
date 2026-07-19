//! Serenity event handler: deployment webhooks and slash-command interactions.

use super::*;

#[serenity::async_trait]
impl EventHandler for DeploymentBot {
    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!("Deployment bot logged in as {}", ready.user.name);
        let commands = deployment_commands();
        if let Some(guild_id) = self.guild_id {
            remove_global_deployment_commands(&ctx).await;
            if let Err(error) = GuildId::new(guild_id)
                .set_commands(&ctx.http, commands)
                .await
            {
                tracing::error!(
                    guild_id,
                    "Failed to sync deployment slash commands: {error}"
                );
            } else {
                tracing::info!(guild_id, "Synced deployment slash commands to guild");
            }
        } else {
            for command in commands {
                if let Err(error) = Command::create_global_command(&ctx.http, command).await {
                    tracing::error!("Failed to register deployment slash command: {error}");
                }
            }
        }
    }

    async fn message(&self, ctx: Context, message: Message) {
        if message.channel_id.get() != self.channel_id {
            return;
        }
        if let Some(event) = deployment_event(&message) {
            tracing::info!(succeeded = event.succeeded, commit = ?event.commit, "Observed deployment webhook");
            let Some(sha) = event.commit.clone().filter(|_| event.succeeded) else {
                return;
            };
            if !valid_sha(&sha) {
                tracing::error!("Deployment webhook contained an invalid SHA");
                return;
            }
            let _deployment_guard = self.deployment_lock.lock().await;
            if self
                .last_event
                .read()
                .await
                .as_ref()
                .is_some_and(|previous| {
                    previous.succeeded && previous.commit.as_deref() == Some(&sha)
                })
            {
                tracing::info!(sha, "Ignoring duplicate build notification");
                return;
            }
            if let Err(error) = self.checkpoint_current_image().await {
                tracing::error!("Could not save deployment checkpoint: {error}");
                return;
            }
            let changelog = match self.current_running_sha().await {
                Ok(current_sha) => match self.changelog(&current_sha, &sha).await {
                    Ok(changelog) => Some(changelog),
                    Err(error) => {
                        tracing::warn!(%error, "Could not build deployment changelog");
                        None
                    }
                },
                Err(error) => {
                    tracing::warn!(%error, "Could not determine previous deployed commit");
                    None
                }
            };
            let commands = match deploy_commands(Some(&sha), &self.docker_network) {
                Ok(commands) => commands,
                Err(error) => {
                    tracing::error!("Could not prepare deployment: {error}");
                    return;
                }
            };
            let mut summary = DeploymentRunSummary {
                container_name: HOUSE_CHATBOT_CONTAINER.into(),
                container_id: None,
            };
            for command in &commands {
                tracing::info!(
                    stage = %command.stage,
                    "Automatic deployment progress"
                );
                let _ = message
                    .channel_id
                    .say(&ctx.http, command.stage.progress_message())
                    .await;
                match run_docker(&command.args()).await {
                    Ok(output) if command.stage.is_health_check() && output != "true" => {
                        tracing::error!(
                            stage = %command.stage,
                            "Automatic deployment stage failed: house-chatbot is not running"
                        );
                        return;
                    }
                    Ok(output) => {
                        if command.stage.is_start() {
                            summary.container_id = Some(output);
                        }
                        tracing::info!(
                            stage = %command.stage,
                            "Automatic deployment stage completed"
                        );
                    }
                    Err(error) => {
                        tracing::error!(
                            stage = %command.stage,
                            "Automatic deployment stage failed: {error}"
                        );
                        return;
                    }
                }
            }
            self.cleanup_old_images(Some(&sha)).await;
            tracing::info!(sha, container = %summary.container_name, container_id = ?summary.container_id, "Automatic deployment completed");
            *self.last_event.write().await = Some(event);
            if let Some(changelog) = changelog {
                let _ = message.channel_id.say(&ctx.http, changelog).await;
            }
            let _ = message
                .channel_id
                .say(&ctx.http, summary.completed_message(&sha))
                .await;
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Component(component) = interaction {
            if component.data.custom_id == "deploy_deny" {
                let response = CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .content("Deployment cancelled.")
                        .components(vec![]),
                );
                let _ = component.create_response(&ctx.http, response).await;
                return;
            }
            let Some(sha) = component.data.custom_id.strip_prefix("deploy_confirm:") else {
                return;
            };
            if !self.deployment_allowed(component.user.id.get()).await {
                let response = CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("You are not allowed to deploy.")
                        .ephemeral(true),
                );
                let _ = component.create_response(&ctx.http, response).await;
                return;
            }
            let response = CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .content("⬇️ Starting deployment…")
                    .components(vec![]),
            );
            if component
                .create_response(&ctx.http, response)
                .await
                .is_err()
            {
                return;
            }
            let commands = if sha == "latest" {
                deploy_commands(None, &self.docker_network)
            } else {
                deploy_commands(Some(sha), &self.docker_network)
            };
            let result = async {
                let _deployment_guard = self.deployment_lock.lock().await;
                self.checkpoint_current_image().await?;
                let commands = commands?;
                let mut summary = DeploymentRunSummary {
                    container_name: HOUSE_CHATBOT_CONTAINER.into(),
                    container_id: None,
                };
                for command in &commands {
                    tracing::info!(
                        stage = %command.stage,
                        "Manual deployment stage started"
                    );
                    component
                        .edit_response(
                            &ctx.http,
                            EditInteractionResponse::new()
                                .content(command.stage.progress_message()),
                        )
                        .await?;
                    let output = match run_docker(&command.args()).await {
                        Ok(output) => output,
                        Err(error) => {
                            tracing::error!(
                                stage = %command.stage,
                                "Manual deployment stage failed: {error}"
                            );
                            return Err(error);
                        }
                    };
                    if command.stage.is_health_check() && output != "true" {
                        anyhow::bail!(
                            "deployment stage `{}` failed: house-chatbot is not running",
                            command.stage
                        );
                    }
                    if command.stage.is_start() {
                        summary.container_id = Some(output);
                    }
                    tracing::info!(
                        stage = %command.stage,
                        "Manual deployment stage completed"
                    );
                }
                self.cleanup_old_images((sha != "latest").then_some(sha))
                    .await;
                anyhow::Ok(summary)
            }
            .await;
            if let Err(error) = &result {
                tracing::error!("Manual deployment failed: {error}");
            }
            let content = match result {
                Ok(summary) => summary.manual_completed_message(sha),
                Err(error) => format!("❌ Deployment failed: {error}"),
            };
            let _ = component
                .edit_response(&ctx.http, EditInteractionResponse::new().content(content))
                .await;
            return;
        }
        let Interaction::Command(command) = interaction else {
            return;
        };
        if command.data.name == "deployment-access" {
            let content = if command.user.id.get() != self.owner_id || self.owner_id == 0 {
                "Only the configured owner can manage deployment access.".to_string()
            } else {
                let option = command.data.options.first();
                match option.map(|option| (option.name.as_str(), &option.value)) {
                    Some(("allow", CommandDataOptionValue::SubCommand(options)))
                    | Some(("revoke", CommandDataOptionValue::SubCommand(options))) => {
                        let user_id = options.iter().find_map(|option| match option.value {
                            CommandDataOptionValue::User(user) => Some(user.get()),
                            _ => None,
                        });
                        match (option.map(|option| option.name.as_str()), user_id) {
                            (Some("allow"), Some(user_id)) => {
                                match self.permissions.allow(user_id, self.owner_id).await {
                                    Ok(()) => {
                                        format!("✅ <@{user_id}> can now deploy and roll back.")
                                    }
                                    Err(error) => {
                                        format!("Could not grant deployment access: {error}")
                                    }
                                }
                            }
                            (Some("revoke"), Some(user_id)) if user_id == self.owner_id => {
                                "The configured owner always has deployment access.".into()
                            }
                            (Some("revoke"), Some(user_id)) => {
                                match self.permissions.revoke(user_id).await {
                                    Ok(()) => {
                                        format!("✅ Revoked deployment access from <@{user_id}>.")
                                    }
                                    Err(error) => {
                                        format!("Could not revoke deployment access: {error}")
                                    }
                                }
                            }
                            _ => "Please specify a user.".into(),
                        }
                    }
                    Some(("list", CommandDataOptionValue::SubCommand(_))) => {
                        match self.permissions.list().await {
                            Ok(users) => {
                                let mut mentions = vec![format!("<@{}> (owner)", self.owner_id)];
                                mentions.extend(
                                    users
                                        .into_iter()
                                        .filter(|user_id| *user_id != self.owner_id)
                                        .map(|user_id| format!("<@{user_id}>")),
                                );
                                format!(
                                    "Users allowed to deploy and roll back:\n{}",
                                    mentions.join("\n")
                                )
                            }
                            Err(error) => format!("Could not list deployment access: {error}"),
                        }
                    }
                    _ => "Choose `allow`, `revoke`, or `list`.".into(),
                }
            };
            let response = CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .ephemeral(true),
            );
            let _ = command.create_response(&ctx.http, response).await;
            return;
        }
        if command.data.name == "deploy" {
            if !self.deployment_allowed(command.user.id.get()).await {
                let response = CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("You are not allowed to deploy.")
                        .ephemeral(true),
                );
                let _ = command.create_response(&ctx.http, response).await;
                return;
            }
            let sha = match command.data.options.first().map(|option| &option.value) {
                Some(CommandDataOptionValue::String(sha)) if valid_sha(sha) => Some(sha.as_str()),
                Some(_) => {
                    let response = CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("SHA must contain 7 to 40 hexadecimal characters.")
                            .ephemeral(true),
                    );
                    let _ = command.create_response(&ctx.http, response).await;
                    return;
                }
                None => None,
            };
            let response = match match sha {
                Some(sha) => self.commits(sha).await,
                None => self
                    .latest_branch_commit()
                    .await
                    .map(|latest| (latest, Vec::new())),
            } {
                Ok((selected, recent)) => {
                    let description = match self.current_running_sha().await {
                        Ok(current_sha) => {
                            match self.changelog(&current_sha, &selected.sha).await {
                                Ok(changelog) => format!(
                                    "{}\n\n{}",
                                    commit_summary(&selected, &recent),
                                    changelog
                                ),
                                Err(error) => format!(
                                    "{}\n\nChangelog unavailable: {error}",
                                    commit_summary(&selected, &recent)
                                ),
                            }
                        }
                        Err(error) => format!(
                            "{}\n\nChangelog unavailable: {error}",
                            commit_summary(&selected, &recent)
                        ),
                    };
                    CreateInteractionResponseMessage::new()
                        .embed(
                            CreateEmbed::new()
                                .title("Confirm deployment")
                                .description(description),
                        )
                        .components(vec![CreateActionRow::Buttons(vec![
                            CreateButton::new(format!(
                                "deploy_confirm:{}",
                                sha.unwrap_or("latest")
                            ))
                            .label("Confirm")
                            .style(ButtonStyle::Success),
                            CreateButton::new("deploy_deny")
                                .label("Deny")
                                .style(ButtonStyle::Danger),
                        ])])
                        .ephemeral(true)
                }
                Err(error) => CreateInteractionResponseMessage::new()
                    .content(format!("Could not find that commit: {error}"))
                    .ephemeral(true),
            };
            let _ = command
                .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                .await;
            return;
        }
        if command.data.name == "update" {
            if !self.deployment_allowed(command.user.id.get()).await {
                let response = CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("You are not allowed to update deployments.")
                        .ephemeral(true),
                );
                let _ = command.create_response(&ctx.http, response).await;
                return;
            }
            let response = CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content("🔎 Checking the running commit against the branch tip…")
                    .ephemeral(true),
            );
            if command.create_response(&ctx.http, response).await.is_err() {
                return;
            }
            let content = match self.update_to_latest().await {
                Ok(message) => message,
                Err(error) => format!("❌ Update failed: {error}"),
            };
            let _ = command
                .edit_response(&ctx.http, EditInteractionResponse::new().content(content))
                .await;
            return;
        }
        if command.data.name != "rollback" {
            return;
        }
        let allowed = self.deployment_allowed(command.user.id.get()).await;
        let reply = if !allowed {
            "You are not allowed to roll back deployments.".to_string()
        } else {
            let _deployment_guard = self.deployment_lock.lock().await;
            match self.rollback().await {
                Ok(message) => message,
                Err(error) => format!("Rollback failed: {error}"),
            }
        };
        let response = CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new()
                .content(reply)
                .ephemeral(true),
        );
        if let Err(error) = command.create_response(&ctx.http, response).await {
            tracing::warn!("Failed to respond to /rollback: {error}");
        }
    }
}
