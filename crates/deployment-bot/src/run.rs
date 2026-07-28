//! Process entry point: client bootstrap, shutdown, and command registration.

use crate::*;

pub async fn run() -> anyhow::Result<()> {
    let token = std::env::var("DEPLOYMENT_DISCORD_BOT_TOKEN")
        .map_err(|_| anyhow::anyhow!("DEPLOYMENT_DISCORD_BOT_TOKEN is not set"))?;
    let owner_id = env_u64("OWNER_DISCORD_ID")?;
    let channel_id = env_u64("DEPLOYMENT_CHANNEL_ID")?;
    let guild_id = optional_env_u64("DEPLOYMENT_GUILD_ID")?;
    let permissions = DeploymentPermissions::connect().await;
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
        permissions,
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
pub(crate) async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = terminate.recv() => {},
    }
}

#[cfg(not(unix))]
pub(crate) async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

pub(crate) async fn shutdown_main_bot() {
    tracing::info!("Stopping managed housebot containers");
    for container in [HOUSE_CHATBOT_CONTAINER, SANDBOXD_CONTAINER] {
        if let Err(error) = run_docker(&["stop", "--time", "10", container]).await {
            tracing::warn!(container, "Could not stop managed container: {error}");
        }
        if let Err(error) = run_docker(&["rm", "--force", container]).await {
            tracing::warn!(container, "Could not remove managed container: {error}");
        }
    }
    tracing::info!("Managed housebot containers stopped");
}

pub(crate) fn env_u64(name: &str) -> anyhow::Result<u64> {
    std::env::var(name)
        .map_err(|_| anyhow::anyhow!("{name} is not set"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("{name} must be a Discord numeric ID"))
}

pub(crate) fn optional_env_u64(name: &str) -> anyhow::Result<Option<u64>> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value
            .parse()
            .map(Some)
            .map_err(|_| anyhow::anyhow!("{name} must be a Discord numeric ID")),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn deployment_commands() -> Vec<CreateCommand> {
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
        CreateCommand::new("deployment-access")
            .description("Manage users allowed to deploy and roll back")
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "allow",
                    "Allow a user to deploy and roll back",
                )
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::User, "user", "User to allow")
                        .required(true),
                ),
            )
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "revoke",
                    "Revoke a user's deployment permission",
                )
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::User, "user", "User to revoke")
                        .required(true),
                ),
            )
            .add_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "list",
                "List users allowed to deploy and roll back",
            )),
    ]
}

pub(crate) async fn remove_global_deployment_commands(ctx: &Context) {
    let commands = match Command::get_global_commands(&ctx.http).await {
        Ok(commands) => commands,
        Err(error) => {
            tracing::error!("Failed to inspect global deployment slash commands: {error}");
            return;
        }
    };

    for command in commands.into_iter().filter(|command| {
        command.name == "deploy"
            || command.name == "rollback"
            || command.name == "update"
            || command.name == "deployment-access"
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
