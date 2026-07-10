//! Small Discord bot that observes deployment webhooks and offers an owner-only rollback.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serenity::all::{
    Command, Context, CreateCommand, CreateInteractionResponse, CreateInteractionResponseMessage,
    EventHandler, GatewayIntents, Interaction, Message, Ready,
};
use serenity::Client;
use tokio::process::Command as ProcessCommand;
use tokio::sync::RwLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeploymentEvent {
    pub succeeded: bool,
    pub commit: Option<String>,
}

pub fn classify_deployment_text(text: &str) -> Option<bool> {
    let normalized = text.to_ascii_lowercase();
    if normalized.contains("deployment succeeded") {
        Some(true)
    } else if normalized.contains("deployment failed") {
        Some(false)
    } else {
        None
    }
}

pub fn deployment_event(message: &Message) -> Option<DeploymentEvent> {
    message.webhook_id?;
    let text = message
        .embeds
        .iter()
        .flat_map(|embed| {
            embed
                .title
                .iter()
                .chain(embed.description.iter())
                .chain(embed.fields.iter().map(|field| &field.value))
        })
        .chain(std::iter::once(&message.content))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let succeeded = classify_deployment_text(&text)?;
    let commit = message
        .embeds
        .iter()
        .flat_map(|embed| &embed.fields)
        .find(|field| field.name.eq_ignore_ascii_case("commit"))
        .map(|field| field.value.trim_matches('`').to_string());
    Some(DeploymentEvent { succeeded, commit })
}

pub fn rollback_allowed(
    owner_id: u64,
    requesting_user: u64,
    channel: u64,
    expected_channel: u64,
) -> bool {
    owner_id != 0 && owner_id == requesting_user && channel == expected_channel
}

#[derive(Clone)]
struct DeploymentBot {
    owner_id: u64,
    channel_id: u64,
    deploy_path: PathBuf,
    last_event: Arc<RwLock<Option<DeploymentEvent>>>,
}

impl DeploymentBot {
    async fn rollback(&self) -> anyhow::Result<String> {
        let checkpoint = self.deploy_path.join(".prev_image_digest");
        let digest = tokio::fs::read_to_string(&checkpoint)
            .await?
            .trim()
            .to_string();
        let commands = rollback_commands(&digest)?;

        for command in &commands {
            let args = command.iter().map(String::as_str).collect::<Vec<_>>();
            run_docker(&args, &self.deploy_path).await?;
        }
        Ok(format!("Rolled house-chatbot back to `{digest}`."))
    }
}

pub fn rollback_commands(digest: &str) -> anyhow::Result<Vec<Vec<String>>> {
    if digest.is_empty()
        || digest == "none"
        || !digest.starts_with("ghcr.io/bushshrub/housebot@sha256:")
    {
        anyhow::bail!("no valid previous deployment checkpoint is available");
    }
    Ok(vec![
        vec!["pull".into(), digest.into()],
        vec![
            "tag".into(),
            digest.into(),
            "ghcr.io/bushshrub/housebot:latest".into(),
        ],
        vec![
            "compose".into(),
            "up".into(),
            "-d".into(),
            "--no-deps".into(),
            "--force-recreate".into(),
            "house-chatbot".into(),
        ],
    ])
}

async fn run_docker(args: &[&str], cwd: &Path) -> anyhow::Result<()> {
    let output = ProcessCommand::new("docker")
        .args(args)
        .current_dir(cwd)
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "docker command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[serenity::async_trait]
impl EventHandler for DeploymentBot {
    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!("Deployment bot logged in as {}", ready.user.name);
        let command = CreateCommand::new("rollback")
            .description("Roll back housebot to the previous deployed image");
        if let Err(error) = Command::create_global_command(&ctx.http, command).await {
            tracing::error!("Failed to register /rollback: {error}");
        }
    }

    async fn message(&self, _ctx: Context, message: Message) {
        if message.channel_id.get() != self.channel_id {
            return;
        }
        if let Some(event) = deployment_event(&message) {
            tracing::info!(succeeded = event.succeeded, commit = ?event.commit, "Observed deployment webhook");
            *self.last_event.write().await = Some(event);
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Interaction::Command(command) = interaction else {
            return;
        };
        if command.data.name != "rollback" {
            return;
        }
        let allowed = rollback_allowed(
            self.owner_id,
            command.user.id.get(),
            command.channel_id.get(),
            self.channel_id,
        );
        let reply = if !allowed {
            "Only the configured owner can roll back from the deployment channel.".to_string()
        } else {
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

pub async fn run() -> anyhow::Result<()> {
    let token = std::env::var("DEPLOYMENT_DISCORD_BOT_TOKEN")
        .map_err(|_| anyhow::anyhow!("DEPLOYMENT_DISCORD_BOT_TOKEN is not set"))?;
    let owner_id = env_u64("OWNER_DISCORD_ID")?;
    let channel_id = env_u64("DEPLOYMENT_CHANNEL_ID")?;
    let deploy_path = std::env::var("DEPLOYMENT_PATH").unwrap_or_else(|_| "/deployment".into());
    let handler = DeploymentBot {
        owner_id,
        channel_id,
        deploy_path: deploy_path.into(),
        last_event: Arc::new(RwLock::new(None)),
    };
    let intents = GatewayIntents::non_privileged() | GatewayIntents::MESSAGE_CONTENT;
    Client::builder(token, intents)
        .event_handler(handler)
        .await?
        .start()
        .await?;
    Ok(())
}

fn env_u64(name: &str) -> anyhow::Result<u64> {
    std::env::var(name)
        .map_err(|_| anyhow::anyhow!("{name} is not set"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("{name} must be a Discord numeric ID"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_is_owner_and_channel_scoped() {
        assert!(rollback_allowed(10, 10, 20, 20));
        assert!(!rollback_allowed(10, 11, 20, 20));
        assert!(!rollback_allowed(10, 10, 21, 20));
        assert!(!rollback_allowed(0, 0, 20, 20));
    }

    #[test]
    fn invalid_numeric_environment_value_is_rejected() {
        std::env::set_var("DEPLOYMENT_BOT_TEST_ID", "not-a-number");
        let error = env_u64("DEPLOYMENT_BOT_TEST_ID").unwrap_err().to_string();
        std::env::remove_var("DEPLOYMENT_BOT_TEST_ID");
        assert!(error.contains("numeric ID"));
    }

    #[test]
    fn deployment_webhook_text_is_classified_strictly() {
        assert_eq!(
            classify_deployment_text("HomeLab deployment succeeded"),
            Some(true)
        );
        assert_eq!(
            classify_deployment_text("HomeLab deployment FAILED"),
            Some(false)
        );
        assert_eq!(classify_deployment_text("build succeeded"), None);
    }

    #[test]
    fn rollback_plan_uses_only_the_checkpoint_digest() {
        let digest = "ghcr.io/bushshrub/housebot@sha256:abc123";
        let commands = rollback_commands(digest).unwrap();
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0], vec!["pull", digest]);
        assert_eq!(commands[2].last().unwrap(), "house-chatbot");
    }

    #[test]
    fn rollback_rejects_tags_and_unrelated_images() {
        assert!(rollback_commands("ghcr.io/bushshrub/housebot:latest").is_err());
        assert!(rollback_commands("ghcr.io/other/image@sha256:abc").is_err());
        assert!(rollback_commands("none").is_err());
    }
}
