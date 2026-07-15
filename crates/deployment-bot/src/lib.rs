//! Small Discord bot that observes deployment webhooks and offers owner-only deployment controls.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use serde::Deserialize;
use serenity::all::{
    ButtonStyle, Command, CommandDataOptionValue, CommandOptionType, Context, CreateActionRow,
    CreateButton, CreateCommand, CreateCommandOption, CreateEmbed, CreateInteractionResponse,
    CreateInteractionResponseMessage, EditInteractionResponse, EventHandler, GatewayIntents,
    GuildId, Interaction, Message, Ready,
};
use serenity::Client;
use tokio::process::Command as ProcessCommand;
use tokio::sync::{Mutex, RwLock};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeploymentEvent {
    pub succeeded: bool,
    pub commit: Option<String>,
}

pub fn classify_deployment_text(text: &str) -> Option<bool> {
    let normalized = text.to_ascii_lowercase();
    if normalized.contains("deployment succeeded") || normalized.contains("build succeeded") {
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
    guild_id: Option<u64>,
    last_event: Arc<RwLock<Option<DeploymentEvent>>>,
    previous_image: Arc<RwLock<Option<String>>>,
    deployment_lock: Arc<Mutex<()>>,
    github_repo: String,
    github_branch: String,
    github_token: Option<String>,
    docker_network: String,
}

const HOUSE_CHATBOT_CONTAINER: &str = "house-chatbot";

mod docker;
mod handler;
use docker::{
    cleanup_old_housebot_images, container_commands_with_env, docker_object_missing, run_docker,
    short_sha, valid_housebot_image, DeploymentRunSummary,
};
pub use docker::{
    container_commands, deploy_commands, deploy_progress, valid_sha, DeploymentCommand,
    DeploymentStage,
};

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

#[derive(Clone, Debug, Deserialize)]
struct GitHubComparison {
    #[serde(default)]
    commits: Vec<GitHubCommit>,
}

impl DeploymentBot {
    async fn cleanup_old_images(&self, sha: Option<&str>) {
        let main = sha
            .map(|sha| format!("ghcr.io/bushshrub/housebot:sha-{sha}"))
            .unwrap_or_else(|| "ghcr.io/bushshrub/housebot:latest".into());
        let previous = self.previous_image.read().await.clone();
        let mut keep = vec![main.as_str()];
        if let Some(previous) = previous.as_deref() {
            keep.push(previous);
        }
        if let Err(error) = cleanup_old_housebot_images(&keep).await {
            tracing::warn!(%error, "Could not clean up old housebot images");
        }
    }

    async fn rollback(&self) -> anyhow::Result<String> {
        let digest = self
            .previous_image
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no previous image is available in this session"))?;
        let commands =
            container_commands_with_env(&digest, &self.docker_network, housebot_env(), false)?;

        for command in &commands {
            let output = run_docker(&command.args()).await?;
            if command.stage.is_health_check() && output != "true" {
                anyhow::bail!("house-chatbot is not running after rollback");
            }
        }
        Ok(format!("Rolled house-chatbot back to `{digest}`."))
    }

    async fn checkpoint_current_image(&self) -> anyhow::Result<()> {
        let image = run_docker(&[
            "inspect",
            "--format={{.Config.Image}}",
            HOUSE_CHATBOT_CONTAINER,
        ])
        .await;
        if let Ok(image) = image {
            if valid_housebot_image(&image) {
                *self.previous_image.write().await = Some(image);
            }
        }
        Ok(())
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

    async fn latest_branch_commit(&self) -> anyhow::Result<GitHubCommit> {
        let client = reqwest::Client::new();
        let url = format!(
            "https://api.github.com/repos/{}/commits/{}",
            self.github_repo, self.github_branch
        );
        let request = client
            .get(url)
            .header("User-Agent", "housebot-deployment-bot")
            .header("Accept", "application/vnd.github+json");
        let request = match &self.github_token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        Ok(request.send().await?.error_for_status()?.json().await?)
    }

    async fn compare_commits(
        &self,
        current_sha: &str,
        target_sha: &str,
    ) -> anyhow::Result<Vec<GitHubCommit>> {
        let client = reqwest::Client::new();
        let url = format!(
            "https://api.github.com/repos/{}/compare/{}...{}",
            self.github_repo, current_sha, target_sha
        );
        let request = client
            .get(url)
            .header("User-Agent", "housebot-deployment-bot")
            .header("Accept", "application/vnd.github+json");
        let request = match &self.github_token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        Ok(request
            .send()
            .await?
            .error_for_status()?
            .json::<GitHubComparison>()
            .await?
            .commits)
    }

    async fn changelog(&self, current_sha: &str, target_sha: &str) -> anyhow::Result<String> {
        let commits = self.compare_commits(current_sha, target_sha).await?;
        Ok(deployment_changelog(current_sha, target_sha, &commits))
    }

    async fn current_running_sha(&self) -> anyhow::Result<String> {
        let image = run_docker(&["inspect", "--format={{.Config.Image}}", "house-chatbot"]).await?;
        let sha = image
            .strip_prefix("ghcr.io/bushshrub/housebot:sha-")
            .ok_or_else(|| {
                anyhow::anyhow!("running house-chatbot image does not contain a commit SHA")
            })?;
        if !valid_sha(sha) {
            anyhow::bail!("running house-chatbot image contains an invalid commit SHA");
        }
        Ok(sha.to_string())
    }

    async fn update_to_latest(&self) -> anyhow::Result<String> {
        let current_sha = match self.current_running_sha().await {
            Ok(sha) => Some(sha),
            Err(error) if docker_object_missing(&error) => None,
            Err(error) => return Err(error),
        };
        let latest = self.latest_branch_commit().await?;
        if current_sha.as_deref() == Some(latest.sha.as_str()) {
            return Ok(format!(
                "✅ Already running the latest `{}` commit on `{}`.",
                short_sha(current_sha.as_deref().expect("current SHA was checked")),
                self.github_branch
            ));
        }

        let _deployment_guard = self.deployment_lock.lock().await;
        let changelog = match current_sha.as_deref() {
            Some(current_sha) => self.changelog(current_sha, &latest.sha).await?,
            None => "**Changelog**\nNo previous housebot container was found; deploying the latest commit."
                .to_string(),
        };
        self.checkpoint_current_image().await?;
        let commands = deploy_commands(Some(&latest.sha), &self.docker_network)?;
        for command in &commands {
            tracing::info!(
                stage = %command.stage,
                "Update deployment stage started"
            );
            let output = run_docker(&command.args()).await?;
            if command.stage.is_health_check() && output != "true" {
                anyhow::bail!("house-chatbot is not running after update deployment");
            }
            tracing::info!(
                stage = %command.stage,
                "Update deployment stage completed"
            );
        }
        let previous = current_sha
            .as_deref()
            .map(short_sha)
            .unwrap_or("no running container");
        Ok(format!(
            "✅ Updated housebot from `{}` to latest `{}` on `{}`.\n\n{}",
            previous,
            short_sha(&latest.sha),
            self.github_branch,
            changelog
        ))
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

pub fn deployment_changelog(
    current_sha: &str,
    target_sha: &str,
    commits: &[GitHubCommit],
) -> String {
    if commits.is_empty() {
        return format!(
            "**Changelog**\n`{}` → `{}`\nNo commits found between these deployments.",
            short_sha(current_sha),
            short_sha(target_sha)
        );
    }

    let mut text = format!(
        "**Changelog since `{}`** ({} commit{})",
        short_sha(current_sha),
        commits.len(),
        if commits.len() == 1 { "" } else { "s" }
    );
    for (shown, commit) in commits.iter().enumerate() {
        let message = commit
            .commit
            .message
            .lines()
            .next()
            .unwrap_or("No commit message");
        let line = format!(
            "\n• [`{}`]({}) — {}",
            short_sha(&commit.sha),
            commit.html_url,
            message
        );
        if text.len() + line.len() > 1_800 {
            text.push_str(&format!(
                "\n• …and {} more commit(s)",
                commits.len() - shown
            ));
            break;
        }
        text.push_str(&line);
    }
    text
}

const HOUSEBOT_ENV_VARS: &[&str] = &[
    "DISCORD_BOT_TOKEN",
    "OWNER_DISCORD_ID",
    "DEPLOYMENT_GUILD_ID",
    "DATABASE_URL",
    "DATABASE_CONNECT_MAX_ATTEMPTS",
    "DATABASE_CONNECT_RETRY_SECS",
    "DATABASE_CONNECT_TIMEOUT_SECS",
    "LLM_BASE_URL",
    "LLM_MODEL",
    "LLM_API_KEY",
    "MAX_HISTORY_TURNS",
    "MAX_CONTEXT_TOKENS",
    "CONVERSATION_IDLE_TIMEOUT",
    "JELLYFIN_URL",
    "JELLYFIN_API_KEY",
    "LLAMA_CPP_URL",
    "LLAMA_CPP_MODEL",
    "GITHUB_APP_ID",
    "GITHUB_APP_PRIVATE_KEY",
    "GITHUB_INSTALLATION_ID",
    "GITHUB_REPO",
];

fn housebot_env() -> Vec<(String, String)> {
    let mut values: HashMap<String, String> = HOUSEBOT_ENV_VARS
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| ((*name).into(), value))
        })
        .collect();

    // Read the mounted deployment configuration at deploy time. This lets an
    // edited .env take effect without restarting the deployment bot itself.
    for path in ["/app/.env", ".env"] {
        if let Ok(contents) = std::fs::read_to_string(path) {
            for (name, value) in parse_dotenv(&contents) {
                if HOUSEBOT_ENV_VARS.contains(&name.as_str()) {
                    values.insert(name, value);
                }
            }
        }
    }

    HOUSEBOT_ENV_VARS
        .iter()
        .filter_map(|name| values.remove(*name).map(|value| ((*name).into(), value)))
        .collect()
}

fn parse_dotenv(contents: &str) -> Vec<(String, String)> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (name, value) = line.split_once('=')?;
            let name = name.trim();
            if name.is_empty() || name.starts_with('#') {
                return None;
            }
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

pub async fn run() -> anyhow::Result<()> {
    let token = std::env::var("DEPLOYMENT_DISCORD_BOT_TOKEN")
        .map_err(|_| anyhow::anyhow!("DEPLOYMENT_DISCORD_BOT_TOKEN is not set"))?;
    let owner_id = env_u64("OWNER_DISCORD_ID")?;
    let channel_id = env_u64("DEPLOYMENT_CHANNEL_ID")?;
    let guild_id = optional_env_u64("DEPLOYMENT_GUILD_ID")?;
    let handler = DeploymentBot {
        owner_id,
        channel_id,
        guild_id,
        last_event: Arc::new(RwLock::new(None)),
        previous_image: Arc::new(RwLock::new(None)),
        deployment_lock: Arc::new(Mutex::new(())),
        github_repo: std::env::var("GITHUB_REPO").unwrap_or_else(|_| "bushshrub/housebot".into()),
        github_branch: std::env::var("GITHUB_BRANCH").unwrap_or_else(|_| "master".into()),
        github_token: std::env::var("GITHUB_TOKEN").ok(),
        docker_network: std::env::var("DOCKER_NETWORK")
            .unwrap_or_else(|_| "house-chatbot_default".into()),
    };
    let intents = GatewayIntents::non_privileged() | GatewayIntents::MESSAGE_CONTENT;
    let mut client = Client::builder(token, intents)
        .event_handler(handler)
        .await?;

    tokio::select! {
        result = client.start() => result?,
        _ = shutdown_signal() => {
            tracing::info!("Deployment bot shutting down and disconnecting from Discord");
            shutdown_main_bot().await;
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = terminate.recv() => {},
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn shutdown_main_bot() {
    tracing::info!("Stopping main house-chatbot container");
    if let Err(error) = run_docker(&["stop", "--time", "10", HOUSE_CHATBOT_CONTAINER]).await {
        tracing::warn!("Could not stop main house-chatbot container: {error}");
    }
    if let Err(error) = run_docker(&["rm", "--force", HOUSE_CHATBOT_CONTAINER]).await {
        tracing::warn!("Could not remove main house-chatbot container: {error}");
    }
    tracing::info!("Main house-chatbot container stopped");
}

fn env_u64(name: &str) -> anyhow::Result<u64> {
    std::env::var(name)
        .map_err(|_| anyhow::anyhow!("{name} is not set"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("{name} must be a Discord numeric ID"))
}

fn optional_env_u64(name: &str) -> anyhow::Result<Option<u64>> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value
            .parse()
            .map(Some)
            .map_err(|_| anyhow::anyhow!("{name} must be a Discord numeric ID")),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn deployment_commands() -> Vec<CreateCommand> {
    vec![
        CreateCommand::new("rollback")
            .description("Roll back housebot to the previous deployed image"),
        CreateCommand::new("update")
            .description("Redeploy the latest commit from the configured branch"),
        CreateCommand::new("deploy")
            .description("Deploy a previously built commit")
            .add_option(
                CreateCommandOption::new(CommandOptionType::String, "sha", "Git commit SHA")
                    .required(false),
            ),
    ]
}

async fn remove_global_deployment_commands(ctx: &Context) {
    let commands = match Command::get_global_commands(&ctx.http).await {
        Ok(commands) => commands,
        Err(error) => {
            tracing::error!("Failed to inspect global deployment slash commands: {error}");
            return;
        }
    };

    for command in commands.into_iter().filter(|command| {
        command.name == "deploy" || command.name == "rollback" || command.name == "update"
    }) {
        if let Err(error) = Command::delete_global_command(&ctx.http, command.id).await {
            tracing::error!(name = %command.name, "Failed to remove global deployment slash command: {error}");
        } else {
            tracing::info!(name = %command.name, "Removed global deployment slash command");
        }
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
