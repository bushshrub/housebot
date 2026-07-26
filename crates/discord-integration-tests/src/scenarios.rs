use std::time::Duration;

use anyhow::Context;
use serenity::all::{ChannelId, CreateMessage, Http, Message, MessageId, UserId};
use tokio::sync::mpsc;
use uuid::Uuid;

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const NEGATIVE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Suite<'a> {
    pub http: &'a Http,
    pub events: &'a mut mpsc::UnboundedReceiver<Message>,
    pub channel_id: ChannelId,
    pub housebot_id: UserId,
    cleanup: Vec<MessageId>,
}

impl<'a> Suite<'a> {
    pub fn new(
        http: &'a Http,
        events: &'a mut mpsc::UnboundedReceiver<Message>,
        channel_id: ChannelId,
        housebot_id: UserId,
    ) -> Self {
        Self {
            http,
            events,
            channel_id,
            housebot_id,
            cleanup: Vec::new(),
        }
    }

    pub async fn run(mut self) -> anyhow::Result<()> {
        let result = self.run_inner().await;
        self.cleanup().await;
        result
    }

    async fn run_inner(&mut self) -> anyhow::Result<()> {
        let basic_response = self.echo().await?;
        self.unmentioned_is_ignored().await?;
        self.reply_followup(&basic_response).await?;
        self.unified_command_adapter().await?;
        self.long_response().await?;
        self.secret_redaction().await?;
        Ok(())
    }

    async fn echo(&mut self) -> anyhow::Result<Message> {
        let nonce = Uuid::new_v4();
        let sent = self
            .send(format!("<@{}> E2E_ECHO:{nonce}", self.housebot_id.get()))
            .await?;
        let response = self
            .wait_for_reply(sent.id, &format!("E2E_OK:{nonce}"), RESPONSE_TIMEOUT)
            .await
            .context("mention/echo scenario")?;
        self.reject_second_reply(sent.id).await?;
        println!("ok: bot mention and deterministic LLM response");
        Ok(response)
    }

    async fn unmentioned_is_ignored(&mut self) -> anyhow::Result<()> {
        let nonce = Uuid::new_v4();
        let sent = self.send(format!("E2E_ECHO:{nonce}")).await?;
        match self.wait_for_reply(sent.id, "", NEGATIVE_TIMEOUT).await {
            Ok(message) => anyhow::bail!(
                "unmentioned bot message received unexpected response {}",
                message.id
            ),
            Err(error) if error.to_string().contains("timed out") => {}
            Err(error) => return Err(error),
        }
        println!("ok: unmentioned bot message ignored");
        Ok(())
    }

    async fn reply_followup(&mut self, prior: &Message) -> anyhow::Result<()> {
        let nonce = Uuid::new_v4();
        let builder = CreateMessage::new()
            .content(format!("<@{}> E2E_REPLY:{nonce}", self.housebot_id.get()))
            .reference_message(prior);
        let sent = self.channel_id.send_message(self.http, builder).await?;
        self.cleanup.push(sent.id);
        self.wait_for_reply(sent.id, &format!("E2E_OK:{nonce}"), RESPONSE_TIMEOUT)
            .await
            .context("reply-followup scenario")?;
        println!("ok: reply follow-up");
        Ok(())
    }

    async fn unified_command_adapter(&mut self) -> anyhow::Result<()> {
        let sent = self
            .send(format!("!/stats <@{}>", self.housebot_id.get()))
            .await?;
        self.wait_for_reply(sent.id, "**Stats for", RESPONSE_TIMEOUT)
            .await
            .context("unified command-adapter scenario")?;
        println!("ok: legacy adapter uses slash-command processor");
        Ok(())
    }

    async fn long_response(&mut self) -> anyhow::Result<()> {
        let nonce = Uuid::new_v4();
        let sent = self
            .send(format!("<@{}> E2E_LONG:{nonce}", self.housebot_id.get()))
            .await?;
        let deadline = tokio::time::Instant::now() + RESPONSE_TIMEOUT;
        let mut combined = String::new();
        let mut first = true;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let message = tokio::time::timeout(remaining, self.events.recv())
                .await
                .context("timed out waiting for long Housebot response")?
                .context("Discord event stream closed")?;
            if message.channel_id != self.channel_id || message.author.id != self.housebot_id {
                continue;
            }
            if first {
                let reference = message
                    .message_reference
                    .as_ref()
                    .and_then(|reference| reference.message_id);
                if reference != Some(sent.id) {
                    continue;
                }
                first = false;
            }
            anyhow::ensure!(
                message.content.chars().count() <= 2_000,
                "Housebot emitted an oversized Discord message"
            );
            self.cleanup.push(message.id);
            combined.push_str(&message.content);
            if combined.contains(&format!("E2E_LONG_END:{nonce}")) {
                break;
            }
        }
        anyhow::ensure!(
            combined.contains(&format!("E2E_LONG_BEGIN:{nonce}")),
            "long response beginning marker missing"
        );
        println!("ok: long response split within Discord limits");
        Ok(())
    }

    async fn secret_redaction(&mut self) -> anyhow::Result<()> {
        let nonce = Uuid::new_v4();
        let fake_secret = std::env::var("E2E_FAKE_SECRET")
            .unwrap_or_else(|_| "housebot-e2e-secret-redacted".to_string());
        let sent = self
            .send(format!("<@{}> E2E_SECRET:{nonce}", self.housebot_id.get()))
            .await?;
        let response = self
            .wait_for_reply(sent.id, "[REDACTED]", RESPONSE_TIMEOUT)
            .await
            .context("secret-redaction scenario")?;
        anyhow::ensure!(
            !response.content.contains(&fake_secret),
            "fake integration secret leaked into Discord"
        );
        println!("ok: fake secret redacted");
        Ok(())
    }

    async fn send(&mut self, content: String) -> anyhow::Result<Message> {
        let message = self.channel_id.say(self.http, content).await?;
        self.cleanup.push(message.id);
        Ok(message)
    }

    async fn wait_for_reply(
        &mut self,
        source_id: MessageId,
        expected: &str,
        timeout: Duration,
    ) -> anyhow::Result<Message> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let message = tokio::time::timeout(remaining, self.events.recv())
                .await
                .context("timed out waiting for correlated Housebot response")?
                .context("Discord event stream closed")?;
            if message.channel_id != self.channel_id || message.author.id != self.housebot_id {
                continue;
            }
            let reference = message
                .message_reference
                .as_ref()
                .and_then(|reference| reference.message_id);
            if reference == Some(source_id) && message.content.contains(expected) {
                self.cleanup.push(message.id);
                return Ok(message);
            }
        }
    }

    async fn reject_second_reply(&mut self, source_id: MessageId) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let Ok(Some(message)) = tokio::time::timeout(remaining, self.events.recv()).await
            else {
                return Ok(());
            };
            if message.author.id == self.housebot_id
                && message
                    .message_reference
                    .as_ref()
                    .and_then(|reference| reference.message_id)
                    == Some(source_id)
            {
                anyhow::bail!("Housebot produced multiple correlated responses");
            }
        }
    }

    async fn cleanup(&self) {
        for message_id in self.cleanup.iter().rev() {
            if let Err(error) = self
                .http
                .delete_message(self.channel_id, *message_id, None)
                .await
            {
                eprintln!("could not delete test message {message_id}: {error}");
            }
        }
    }
}
