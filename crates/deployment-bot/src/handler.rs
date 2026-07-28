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
                match run_deployment_command(command).await {
                    Ok(output) if command.stage.is_health_check() && output != "true" => {
                        tracing::error!(
                            stage = %command.stage,
                            "Automatic deployment stage failed: house-chatbot is not running"
                        );
                        let content = format!(
                            "❌ Automatic deployment of `{}` failed at `{}`: house-chatbot is not running.",
                            short_sha(&sha),
                            command.stage
                        );
                        if let Err(send_error) = message.channel_id.say(&ctx.http, content).await {
                            tracing::warn!(%send_error, "Could not report automatic deployment failure to Discord");
                        }
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
                        let content = truncate_for_discord(format!(
                            "❌ Automatic deployment of `{}` failed at `{}`: {error}",
                            short_sha(&sha),
                            command.stage
                        ));
                        if let Err(send_error) = message.channel_id.say(&ctx.http, content).await {
                            tracing::warn!(%send_error, "Could not report automatic deployment failure to Discord");
                        }
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
        self.on_interaction(ctx, interaction).await;
    }
}

const DISCORD_MESSAGE_LIMIT: usize = 2000;

fn truncate_for_discord(content: String) -> String {
    if content.chars().count() <= DISCORD_MESSAGE_LIMIT {
        return content;
    }
    let mut truncated: String = content.chars().take(DISCORD_MESSAGE_LIMIT - 1).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
#[path = "handler_tests.rs"]
mod tests;
