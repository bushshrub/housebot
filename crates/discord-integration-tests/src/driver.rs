use std::sync::Mutex;

use anyhow::Context as _;
use reqwest::StatusCode;
use serde::Deserialize;
use serenity::all::{ChannelId, Context, EventHandler, GatewayIntents, Message, Ready, UserId};
use serenity::http::Http;
use serenity::Client;
use tokio::sync::{mpsc, oneshot};
use tokio_postgres::NoTls;

use crate::scenarios::Suite;

const API: &str = "https://discord.com/api/v10";

#[derive(Deserialize)]
struct DiscordUser {
    id: String,
}

#[derive(Deserialize)]
struct DiscordChannel {
    id: String,
    guild_id: Option<String>,
    #[serde(rename = "type")]
    kind: u8,
}

#[derive(Deserialize)]
struct ApplicationCommand {
    name: String,
    #[serde(default)]
    options: Vec<serde_json::Value>,
}

struct Handler {
    messages: mpsc::UnboundedSender<Message>,
    ready: Mutex<Option<oneshot::Sender<()>>>,
}

#[serenity::async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        println!(
            "driver logged in as {} ({})",
            ready.user.name, ready.user.id
        );
        if let Some(sender) = self.ready.lock().expect("ready mutex poisoned").take() {
            let _ = sender.send(());
        }
    }

    async fn message(&self, _ctx: Context, message: Message) {
        let _ = self.messages.send(message);
    }
}

pub async fn run() -> anyhow::Result<()> {
    let driver_token = required("INTEGRATION_TEST_DRIVER_DISCORD_TOKEN")?;
    let housebot_token = required("INTEGRATION_TEST_DISCORD_TOKEN")?;
    let database_url = required("DATABASE_URL")?;
    let api = reqwest::Client::new();

    let driver_user = current_user(&api, &driver_token).await?;
    let housebot_user = current_user(&api, &housebot_token).await?;
    let (guild_id, channel_id) =
        discover_test_channel(&api, &driver_token, &housebot_user.id).await?;
    seed_test_config(&database_url, guild_id, channel_id, driver_user.id.parse()?).await?;

    println!("using guild {guild_id}, channel {channel_id}");
    let (message_tx, mut message_rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = oneshot::channel();
    let handler = Handler {
        messages: message_tx,
        ready: Mutex::new(Some(ready_tx)),
    };
    let intents = GatewayIntents::GUILDS | GatewayIntents::GUILD_MESSAGES;
    let mut client = Client::builder(&driver_token, intents)
        .event_handler(handler)
        .await?;
    let shard_manager = client.shard_manager.clone();
    let client_task = tokio::spawn(async move { client.start().await });
    tokio::time::timeout(std::time::Duration::from_secs(30), ready_rx)
        .await
        .context("driver did not become ready")??;

    verify_slash_commands(&api, &housebot_token, &housebot_user.id, guild_id).await?;
    let http = Http::new(&driver_token);
    let result = Suite::new(
        &http,
        &mut message_rx,
        ChannelId::new(channel_id),
        UserId::new(housebot_user.id.parse()?),
    )
    .run()
    .await;
    shard_manager.shutdown_all().await;
    let _ = client_task.await;
    result
}

async fn verify_slash_commands(
    client: &reqwest::Client,
    token: &str,
    application_id: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let global: Vec<ApplicationCommand> = discord_get(
            client,
            token,
            &format!("/applications/{application_id}/commands"),
        )
        .await?;
        let guild: Vec<ApplicationCommand> = discord_get(
            client,
            token,
            &format!("/applications/{application_id}/guilds/{guild_id}/commands"),
        )
        .await?;
        let stats_registered = global.iter().any(|command| command.name == "stats");
        let labs = guild.iter().find(|command| command.name == "labs");
        if stats_registered && labs.is_some_and(|command| !command.options.is_empty()) {
            println!("ok: global and guild slash-command schemas registered");
            return Ok(());
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "Housebot slash-command schemas were not registered within 30 seconds"
        );
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

async fn current_user(client: &reqwest::Client, token: &str) -> anyhow::Result<DiscordUser> {
    client
        .get(format!("{API}/users/@me"))
        .header("authorization", format!("Bot {token}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("could not identify Discord bot token")
}

async fn discover_test_channel(
    client: &reqwest::Client,
    token: &str,
    housebot_id: &str,
) -> anyhow::Result<(u64, u64)> {
    let channel_id = required("INTEGRATION_TEST_CHANNEL_ID")?;
    let channel: DiscordChannel =
        discord_get(client, token, &format!("/channels/{channel_id}")).await?;
    anyhow::ensure!(
        channel.kind == 0 || channel.kind == 5,
        "INTEGRATION_TEST_CHANNEL_ID is not a guild text channel"
    );
    let guild_id = channel
        .guild_id
        .context("INTEGRATION_TEST_CHANNEL_ID does not belong to a guild")?;
    let member = client
        .get(format!("{API}/guilds/{guild_id}/members/{housebot_id}"))
        .header("authorization", format!("Bot {token}"))
        .send()
        .await?;
    anyhow::ensure!(
        member.status() == StatusCode::OK,
        "Housebot is not a member of the configured integration-test guild"
    );
    Ok((guild_id.parse()?, channel.id.parse()?))
}

async fn discord_get<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    token: &str,
    path: &str,
) -> anyhow::Result<T> {
    Ok(client
        .get(format!("{API}{path}"))
        .header("authorization", format!("Bot {token}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn seed_test_config(
    database_url: &str,
    guild_id: u64,
    channel_id: u64,
    driver_id: u64,
) -> anyhow::Result<()> {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls).await?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            eprintln!("test database connection closed: {error}");
        }
    });
    let server_value = serde_json::json!({
        "allowed_channel_ids": [channel_id],
        "respond_to_bot_pings": true
    })
    .to_string();
    let user_value = serde_json::json!({
        "progress_updates_enabled": false,
        "deep_memory_enabled": false
    })
    .to_string();
    for (key, value) in [
        (format!("server:{guild_id}"), server_value),
        (format!("user:{driver_id}"), user_value),
    ] {
        client
            .execute(
                "INSERT INTO bot_config (key, value) VALUES ($1, $2) \
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()",
                &[&key, &value],
            )
            .await?;
    }
    Ok(())
}

fn required(name: &str) -> anyhow::Result<String> {
    std::env::var(name).with_context(|| format!("{name} is not set"))
}
