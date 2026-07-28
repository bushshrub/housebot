//! Small Discord bot that observes deployment webhooks and offers database-backed deployment controls.

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

mod permissions;
use permissions::DeploymentPermissions;

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
    permissions: DeploymentPermissions,
}

const HOUSE_CHATBOT_CONTAINER: &str = "house-chatbot";
const SANDBOXD_CONTAINER: &str = "housebot-sandboxd";

mod docker;
mod handler;
use docker::{
    cleanup_old_images, container_commands_with_env, docker_object_missing, run_deployment_command,
    run_docker, short_sha, valid_housebot_image, DeploymentRunSummary,
};
pub use docker::{
    container_commands, deploy_commands, deploy_progress, valid_sha, DeploymentCommand,
    DeploymentStage,
};

mod changelog;
mod env;
mod run;

pub(crate) use changelog::GitHubComparison;
pub use changelog::*;
pub(crate) use env::*;
pub use run::*;

impl DeploymentBot {
    async fn deployment_allowed(&self, user_id: u64) -> bool {
        if self.owner_id != 0 && user_id == self.owner_id {
            return true;
        }
        match self.permissions.contains(user_id).await {
            Ok(allowed) => allowed,
            Err(error) => {
                tracing::error!(%error, user_id, "Could not check deployment permission");
                false
            }
        }
    }

    async fn cleanup_old_images(&self, sha: Option<&str>) {
        let main = sha
            .map(|sha| format!("ghcr.io/bushshrub/housebot:sha-{sha}"))
            .unwrap_or_else(|| "ghcr.io/bushshrub/housebot:latest".into());
        let sandboxd = sha
            .map(|sha| format!("ghcr.io/bushshrub/housebot/sandboxd:sha-{sha}"))
            .unwrap_or_else(|| "ghcr.io/bushshrub/housebot/sandboxd:latest".into());
        let sandbox = sha
            .map(|sha| format!("ghcr.io/bushshrub/housebot/sandbox:sha-{sha}"))
            .unwrap_or_else(|| "ghcr.io/bushshrub/housebot/sandbox:latest".into());
        let previous = self.previous_image.read().await.clone();
        let mut keep = vec![main.as_str(), sandboxd.as_str(), sandbox.as_str()];
        if let Some(previous) = previous.as_deref() {
            keep.push(previous);
        }
        if let Err(error) = cleanup_old_images(&keep).await {
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
            let output = run_deployment_command(command).await?;
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
            let output = run_deployment_command(command).await?;
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
