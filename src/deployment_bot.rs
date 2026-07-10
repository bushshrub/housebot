//! Small Discord bot that observes deployment webhooks and offers an owner-only rollback.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serenity::all::{
    ButtonStyle, Command, CommandDataOptionValue, CommandOptionType, Context, CreateActionRow,
    CreateButton, CreateCommand, CreateCommandOption, CreateEmbed, CreateInteractionResponse,
    CreateInteractionResponseMessage, EditInteractionResponse, EventHandler, GatewayIntents,
    Interaction, Message, Ready,
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
    github_repo: String,
    github_token: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct GitHubCommit {
    pub sha: String,
    pub html_url: String,
    pub commit: GitHubCommitDetails,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct GitHubCommitDetails {
    pub message: String,
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

    async fn commits(&self, sha: &str) -> anyhow::Result<(GitHubCommit, Vec<GitHubCommit>)> {
        let client = reqwest::Client::new();
        let base = format!("https://api.github.com/repos/{}", self.github_repo);
        let request = |url: String| {
            let request = client
                .get(url)
                .header("User-Agent", "housebot-deployment-bot")
                .header("Accept", "application/vnd.github+json");
            match &self.github_token {
                Some(token) => request.bearer_auth(token),
                None => request,
            }
        };
        let selected: GitHubCommit = request(format!("{base}/commits/{sha}"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let recent = request(format!("{base}/commits?sha={}&per_page=4", selected.sha))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok((selected, recent))
    }
}

pub fn valid_sha(sha: &str) -> bool {
    (7..=40).contains(&sha.len()) && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn deploy_commands(sha: &str) -> anyhow::Result<Vec<Vec<String>>> {
    if !valid_sha(sha) {
        anyhow::bail!("SHA must contain 7 to 40 hexadecimal characters");
    }
    let main = format!("ghcr.io/bushshrub/housebot:sha-{sha}");
    let sandbox = format!("ghcr.io/bushshrub/housebot/sandbox:sha-{sha}");
    Ok(vec![
        vec!["pull".into(), main.clone()],
        vec!["pull".into(), sandbox.clone()],
        vec![
            "tag".into(),
            main,
            "ghcr.io/bushshrub/housebot:latest".into(),
        ],
        vec![
            "tag".into(),
            sandbox,
            "ghcr.io/bushshrub/housebot/sandbox:latest".into(),
        ],
        vec![
            "compose".into(),
            "up".into(),
            "-d".into(),
            "--no-deps".into(),
            "--force-recreate".into(),
            "house-chatbot".into(),
        ],
        vec![
            "compose".into(),
            "ps".into(),
            "--status".into(),
            "running".into(),
            "--quiet".into(),
            "house-chatbot".into(),
        ],
    ])
}

pub fn deploy_progress(index: usize) -> &'static str {
    match index {
        0 => "⬇️ Pulling housebot image…",
        1 => "⬇️ Pulling sandbox image…",
        2 | 3 => "🏷️ Selecting requested images…",
        4 => "🚀 Recreating housebot…",
        _ => "🩺 Checking container state…",
    }
}

pub fn commit_summary(selected: &GitHubCommit, recent: &[GitHubCommit]) -> String {
    let first_line = selected
        .commit
        .message
        .lines()
        .next()
        .unwrap_or("No commit message");
    let mut text = format!(
        "Deploying [`{}`]({}) — {}\n\n**Recent commits:**",
        short_sha(&selected.sha),
        selected.html_url,
        first_line
    );
    for commit in recent
        .iter()
        .filter(|commit| commit.sha != selected.sha)
        .take(3)
    {
        let message = commit
            .commit
            .message
            .lines()
            .next()
            .unwrap_or("No commit message");
        text.push_str(&format!(
            "\n• [`{}`]({}) — {}",
            short_sha(&commit.sha),
            commit.html_url,
            message
        ));
    }
    text
}

fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
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

async fn run_docker(args: &[&str], cwd: &Path) -> anyhow::Result<String> {
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
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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
        let deploy = CreateCommand::new("deploy")
            .description("Deploy a previously built commit")
            .add_option(
                CreateCommandOption::new(CommandOptionType::String, "sha", "Git commit SHA")
                    .required(true),
            );
        if let Err(error) = Command::create_global_command(&ctx.http, deploy).await {
            tracing::error!("Failed to register /deploy: {error}");
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
            if !rollback_allowed(
                self.owner_id,
                component.user.id.get(),
                component.channel_id.get(),
                self.channel_id,
            ) {
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
            let commands = deploy_commands(sha);
            let result = async {
                let commands = commands?;
                for (index, command) in commands.iter().enumerate() {
                    component
                        .edit_response(
                            &ctx.http,
                            EditInteractionResponse::new().content(deploy_progress(index)),
                        )
                        .await?;
                    let args = command.iter().map(String::as_str).collect::<Vec<_>>();
                    let output = run_docker(&args, &self.deploy_path).await?;
                    if index == commands.len() - 1 && output.is_empty() {
                        anyhow::bail!("house-chatbot is not running after deployment");
                    }
                }
                anyhow::Ok(())
            }
            .await;
            let content = match result {
                Ok(()) => format!("✅ Deployment of `{}` completed.", short_sha(sha)),
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
        if command.data.name == "deploy" {
            if !rollback_allowed(
                self.owner_id,
                command.user.id.get(),
                command.channel_id.get(),
                self.channel_id,
            ) {
                let response = CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("Only the configured owner can deploy from this channel.")
                        .ephemeral(true),
                );
                let _ = command.create_response(&ctx.http, response).await;
                return;
            }
            let sha = match command.data.options.first().map(|option| &option.value) {
                Some(CommandDataOptionValue::String(sha)) if valid_sha(sha) => sha,
                _ => {
                    let response = CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("SHA must contain 7 to 40 hexadecimal characters.")
                            .ephemeral(true),
                    );
                    let _ = command.create_response(&ctx.http, response).await;
                    return;
                }
            };
            let response = match self.commits(sha).await {
                Ok((selected, recent)) => CreateInteractionResponseMessage::new()
                    .embed(
                        CreateEmbed::new()
                            .title("Confirm deployment")
                            .description(commit_summary(&selected, &recent)),
                    )
                    .components(vec![CreateActionRow::Buttons(vec![
                        CreateButton::new(format!("deploy_confirm:{}", selected.sha))
                            .label("Confirm")
                            .style(ButtonStyle::Success),
                        CreateButton::new("deploy_deny")
                            .label("Deny")
                            .style(ButtonStyle::Danger),
                    ])])
                    .ephemeral(true),
                Err(error) => CreateInteractionResponseMessage::new()
                    .content(format!("Could not find that commit: {error}"))
                    .ephemeral(true),
            };
            let _ = command
                .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                .await;
            return;
        }
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
        github_repo: std::env::var("GITHUB_REPO").unwrap_or_else(|_| "bushshrub/housebot".into()),
        github_token: std::env::var("GITHUB_TOKEN").ok(),
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

    #[test]
    fn deploy_plan_is_sha_scoped_and_rejects_injection() {
        let commands = deploy_commands("abcdef123456").unwrap();
        assert_eq!(commands.len(), 6);
        assert!(commands[0][1].ends_with(":sha-abcdef123456"));
        assert!(deploy_commands("latest").is_err());
        assert!(deploy_commands("abcdef;reboot").is_err());
    }

    #[test]
    fn commit_summary_has_links_messages_and_alternatives() {
        let commit = |sha: &str, message: &str| GitHubCommit {
            sha: sha.into(),
            html_url: format!("https://github.com/example/repo/commit/{sha}"),
            commit: GitHubCommitDetails {
                message: message.into(),
            },
        };
        let selected = commit("abcdef1234", "selected commit\nbody");
        let summary = commit_summary(
            &selected,
            &[selected.clone(), commit("1234567890", "older")],
        );
        assert!(summary.contains("[`abcdef1`](https://github.com/example/repo/commit/abcdef1234)"));
        assert!(summary.contains("selected commit"));
        assert!(summary.contains("older"));
    }
}
